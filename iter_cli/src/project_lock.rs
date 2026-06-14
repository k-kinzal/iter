//! Project-scoped advisory lock for `iter compose up -d`.
//!
//! Closes the TOCTOU window between
//! [`crate::find_active_orchestrator`] and the actual orchestrator
//! double-fork: without this lock two `compose up -d` invocations for
//! the same slug can both observe "no orchestrator running" and both
//! proceed to spawn one, leaving the project owned by two competing
//! orchestrators (Codex iter-9 Major 1).
//!
//! The lock is a single-byte file at
//! `~/.iter/compose/locks/<slug>.lock` held under `flock(LOCK_EX |
//! LOCK_NB)`. Releasing happens automatically when the returned
//! [`ProjectLock`] is dropped (the kernel releases the flock when the
//! last fd referring to the file is closed).
//!
//! The lock file is **not** removed on release: a stale empty file is
//! cheap, and `unlink → open → flock` would re-introduce a TOCTOU
//! window where two callers race past `unlink` into separate `open`
//! calls.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use thiserror::Error;

use crate::process::{ProcessError, proc_root_default};

/// Errors returned by [`acquire_project_lock`].
#[derive(Debug, Error)]
pub(crate) enum ProjectLockError {
    /// Resolving `~/.iter/proc/` (and from it the compose lock root)
    /// failed — typically because `$HOME` is unset.
    #[error("could not resolve iter root: {0}")]
    Root(#[source] ProcessError),
    /// Creating `~/.iter/compose/locks/` failed.
    #[error("creating compose lock directory {path:?}: {source}")]
    Mkdir {
        /// The directory path we tried to create.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Opening the per-project lock file failed.
    #[error("opening project lock {path:?}: {source}")]
    Open {
        /// The lock file path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Another `iter compose up` invocation already holds the lock for
    /// this project. The caller should refuse to spawn a second
    /// orchestrator and surface the conflict to the user.
    #[error("project {project:?} is being started by another `iter compose up` invocation")]
    AlreadyHeld {
        /// Project slug we tried to lock.
        project: String,
    },
    /// `flock(2)` failed for some reason other than `EWOULDBLOCK`.
    #[error("flock on {path:?}: {source}")]
    Flock {
        /// The lock file path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}

/// RAII guard for an exclusive project lock. Drop releases the
/// underlying flock when the held [`File`] is closed.
#[derive(Debug)]
#[must_use = "drop releases the project lock; assign to a binding"]
pub(crate) struct ProjectLock {
    _file: File,
    project: String,
}

impl ProjectLock {
    /// Project slug this lock guards. Useful for diagnostics that want
    /// to confirm which slug was actually locked (e.g. after override
    /// resolution).
    #[must_use]
    pub(crate) fn project(&self) -> &str {
        &self.project
    }
}

/// Acquire the per-project lock for `project`.
///
/// Returns immediately with [`ProjectLockError::AlreadyHeld`] if
/// another process already holds the lock — `compose up -d` is meant
/// to be a fast "is anyone else starting this project" check, not a
/// blocking serialiser.
///
/// # Errors
///
/// * [`ProjectLockError::Root`] — `$HOME` cannot be resolved.
/// * [`ProjectLockError::Mkdir`] — the `~/.iter/compose/locks/` tree
///   cannot be created.
/// * [`ProjectLockError::Open`] — the per-project lock file cannot be
///   opened (permissions, ENOSPC, …).
/// * [`ProjectLockError::AlreadyHeld`] — another `compose up` is
///   currently inside its lock-protected critical section.
/// * [`ProjectLockError::Flock`] — `flock(2)` failed for an unexpected
///   reason.
pub(crate) fn acquire_project_lock(project: &str) -> Result<ProjectLock, ProjectLockError> {
    let lock_path = lock_path_for(project)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        // `lock_path_for` always returns `<iter_root>/compose/locks/<slug>.lock`,
        // so the parent is always present. Fall back to `Path::new(".")` rather
        // than `unwrap` so static analysis cannot construct a panic path.
        let dir = lock_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|source| ProjectLockError::Mkdir {
                path: dir.to_path_buf(),
                source,
            })?;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|source| ProjectLockError::Open {
                path: lock_path.clone(),
                source,
            })?;
        flock_nonblock(&file).map_err(|source| {
            // On Linux and macOS `EAGAIN == EWOULDBLOCK`, but some
            // BSDs split them — `flock(2)` can return either. Match
            // the `io::ErrorKind` instead of a single libc constant
            // so the check stays portable across platforms (Codex
            // iter-10 Minor C).
            if source.kind() == io::ErrorKind::WouldBlock {
                ProjectLockError::AlreadyHeld {
                    project: project.to_owned(),
                }
            } else {
                ProjectLockError::Flock {
                    path: lock_path.clone(),
                    source,
                }
            }
        })?;
        Ok(ProjectLock {
            _file: file,
            project: project.to_owned(),
        })
    }

    #[cfg(not(unix))]
    {
        // Non-unix platforms have no `flock` equivalent in the std
        // library; fall back to opening (and holding) the file so
        // *something* exists, even if the lock semantic is degraded.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| ProjectLockError::Open {
                path: lock_path.clone(),
                source,
            })?;
        Ok(ProjectLock {
            _file: file,
            project: project.to_owned(),
        })
    }
}

fn lock_path_for(project: &str) -> Result<PathBuf, ProjectLockError> {
    let proc_root = proc_root_default().map_err(ProjectLockError::Root)?;
    // `proc_root_default` returns `<iter_root>/proc`; step up to get
    // the `<iter_root>/` we want to anchor `compose/locks/` under. We
    // never expect the parent to be missing for an absolute home path,
    // but if it ever is we fall back to `proc_root` itself rather than
    // panic.
    let iter_root = proc_root.parent().unwrap_or(proc_root.as_path());
    Ok(iter_root
        .join("compose")
        .join("locks")
        .join(format!("{project}.lock")))
}

#[cfg(unix)]
fn flock_nonblock(file: &File) -> io::Result<()> {
    // SAFETY: `file.as_raw_fd()` is a live kernel file descriptor for
    // the lifetime of the `&File` borrow; `libc::flock` reads only the
    // fd and op flags.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Drive the lock against an explicit lock-file path (no env
    /// mutation). Mirrors what [`acquire_project_lock`] does, minus
    /// the `lock_path_for` indirection — keeps the tests free of
    /// process-wide `HOME` mutation, which the workspace lints as
    /// `unsafe` and which would race anything else touching the env.
    fn acquire_at(path: &std::path::Path, project: &str) -> Result<ProjectLock, ProjectLockError> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        let dir = path.parent().expect("test path always has a parent");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|source| ProjectLockError::Mkdir {
                path: dir.to_path_buf(),
                source,
            })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|source| ProjectLockError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        flock_nonblock(&file).map_err(|source| {
            // On Linux and macOS `EAGAIN == EWOULDBLOCK`, but some
            // BSDs split them — `flock(2)` can return either. Match
            // the `io::ErrorKind` instead of a single libc constant
            // so the check stays portable across platforms (Codex
            // iter-10 Minor C).
            if source.kind() == io::ErrorKind::WouldBlock {
                ProjectLockError::AlreadyHeld {
                    project: project.to_owned(),
                }
            } else {
                ProjectLockError::Flock {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        Ok(ProjectLock {
            _file: file,
            project: project.to_owned(),
        })
    }

    #[test]
    fn first_acquire_succeeds_second_fails_third_succeeds_after_drop() {
        let home = TempDir::new().unwrap();
        let path = home.path().join("locks").join("demo.lock");
        let lock = acquire_at(&path, "demo").expect("first acquire");
        assert_eq!(lock.project(), "demo");

        let conflict = acquire_at(&path, "demo");
        assert!(
            matches!(
                conflict,
                Err(ProjectLockError::AlreadyHeld { ref project }) if project == "demo"
            ),
            "expected AlreadyHeld, got {conflict:?}"
        );

        drop(lock);
        let _re = acquire_at(&path, "demo").expect("re-acquire after drop");
    }

    #[test]
    fn distinct_projects_do_not_conflict() {
        let home = TempDir::new().unwrap();
        let alpha_path = home.path().join("locks").join("alpha.lock");
        let beta_path = home.path().join("locks").join("beta.lock");
        let a = acquire_at(&alpha_path, "alpha").expect("alpha");
        let b = acquire_at(&beta_path, "beta").expect("beta");
        drop((a, b));
    }

    #[test]
    fn lock_path_for_anchors_under_iter_compose_locks() {
        // Sanity-check the path shape — proves the production helper
        // and the test helper agree on directory layout without
        // mutating HOME.
        let path = lock_path_for("demo").expect("HOME is set in CI");
        let s = path.to_string_lossy();
        assert!(s.ends_with("/compose/locks/demo.lock"), "path = {s}");
    }
}
