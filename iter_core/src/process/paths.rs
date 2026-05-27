//! Canonical filesystem layout under `~/.iter/proc/<id>/`.
//!
//! Every directory created here is mode `0o700`; every per-id file is mode
//! `0o600`. All persistent file-descriptors are flagged `O_CLOEXEC` so they
//! are not leaked into spawned subprocesses.
//!
//! [`ProcPaths`] bundles the absolute directory path with an
//! [`OwnedFd`] obtained from `open(dir, O_DIRECTORY|O_CLOEXEC|O_RDONLY)`.
//! Holding the `dirfd` lets every other module reach the contents through
//! `openat` / `linkat` / `unlinkat` / `fstatat`, which (a) eliminates a class
//! of TOCTOU bugs where the directory could be replaced while we walk a
//! string path and (b) gives `process_dir_vanished` an authoritative signal
//! (`fstat(dirfd).nlink == 0`).
//!
//! # Why `Arc<ProcPaths>` is the public form
//!
//! `tokio::task::spawn_blocking` requires `'static` closures. Passing
//! `&ProcPaths` would tie the closure to the surrounding stack frame; passing
//! `Arc<ProcPaths>` lets the closure clone its own owning handle and satisfy
//! the bound. Every async wrapper in the subsystem therefore consumes
//! `Arc<ProcPaths>` and clones into `spawn_blocking`.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::process::error::ProcessError;
use crate::process::id::ProcessId;

/// Mode for every directory under `~/.iter/proc/`.
pub const DIR_MODE: u32 = 0o700;
/// Mode for every per-id file (`pid`, `status`, `bootstrap_token`, ...).
pub const FILE_MODE: u32 = 0o600;

/// File names written under each `~/.iter/proc/<id>/` directory.
pub mod names {
    /// Atomic-published pid file (`linkat`-only — see `pid_file.rs`).
    pub const PID: &str = "pid";
    /// Temporary file used during the pid publication's `linkat` step.
    pub const PID_TMP: &str = ".pid.tmp";
    /// Lifecycle state token (`initializing` / `running` / `stopped` / …).
    pub const STATUS: &str = "status";
    /// Anti-accidental-adoption token (16 bytes, on disk as 32 hex chars).
    pub const BOOTSTRAP_TOKEN: &str = "bootstrap_token";
    /// Plain-text registered name.
    pub const NAME: &str = "name";
    /// Absolute path of the `Iterfile` that was loaded.
    pub const ITERFILE: &str = "iterfile";
    /// CLI subcommand verb (`"run"`, `"compose up"`, …).
    pub const SUBCOMMAND: &str = "subcommand";
    /// RFC 3339 timestamp of session creation.
    pub const STARTED_AT: &str = "started_at";
    /// JSON-serialised [`ProcessMetadata`](crate::process::metadata::ProcessMetadata).
    pub const META: &str = "meta.json";
    /// Append-only NDJSON of `{ts, stream, line}` records — the unified
    /// docker-logs-parity stream that captures everything the worker
    /// process emits (agent stdout/stderr, runner tracing, lifecycle
    /// events). Written by [`crate::process::log::ProcessLogSink`] under
    /// [`OutputPolicy::LogOnly`](crate::process::log::OutputPolicy::LogOnly)
    /// and read by `iter logs` / `iter attach`.
    pub const LOG_NDJSON: &str = "log.ndjson";
}

/// Subdirectory under `~/.iter/proc/` reserved for `name → ulid` lock files.
pub const LOCKS_SUBDIR: &str = ".locks";

/// Canonical path of `~/.iter/proc/`. Caller is responsible for ensuring
/// # Errors
///
/// Returns an error if the operation fails.
/// `~/.iter/proc/` and `~/.iter/proc/.locks/` exist with mode `0o700`.
pub fn proc_root_default() -> Result<PathBuf, ProcessError> {
    let home = home_dir().ok_or_else(|| {
        ProcessError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve home directory",
        ))
    })?;
    Ok(home.join(".iter").join("proc"))
}

/// Per-id directory layout, owning a `dirfd` for relative-path syscalls.
///
/// Constructed by `ProcessSession::create_initial` (foreground) or
/// `Spawner::register_for_detached` (parent) and shared via `Arc<ProcPaths>`.
#[derive(Debug)]
pub struct ProcPaths {
    id: ProcessId,
    dir: PathBuf,
    dirfd: OwnedFd,
}

impl ProcPaths {
    /// Wrap an existing directory + its `dirfd`. Callers must ensure that:
    ///
    /// - `dir` already exists with mode `0o700`.
    /// - `dirfd` was opened with `O_DIRECTORY | O_CLOEXEC | O_RDONLY`.
    /// - `dir` and `dirfd` refer to the same on-disk inode.
    ///
    /// This is intended for internal use; the public construction sites are
    /// [`Self::create_for_new_id`] and [`Self::open_existing`].
    #[must_use]
    pub fn from_parts(id: ProcessId, dir: PathBuf, dirfd: OwnedFd) -> Self {
        Self { id, dir, dirfd }
    }

    /// Create `<root>/<id>/` with mode `0o700`, then open it as a `dirfd`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// `root` must already exist (typically `~/.iter/proc/`).
    pub fn create_for_new_id(root: &Path, id: ProcessId) -> Result<Arc<Self>, ProcessError> {
        ensure_dir_with_mode(root, DIR_MODE)?;
        let dir = root.join(id.to_string());
        ensure_dir_with_mode(&dir, DIR_MODE)?;
        let dirfd = open_dirfd(&dir)?;
        Ok(Arc::new(Self::from_parts(id, dir, dirfd)))
    }

    /// Open an existing `<root>/<id>/` as a `dirfd` without creating the dir.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn open_existing(root: &Path, id: ProcessId) -> Result<Arc<Self>, ProcessError> {
        let dir = root.join(id.to_string());
        let dirfd = open_dirfd(&dir)?;
        Ok(Arc::new(Self::from_parts(id, dir, dirfd)))
    }

    /// The `ProcessId` this layout was opened for.
    #[must_use]
    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// Absolute directory path. Suitable for `Display`-level diagnostics.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `dirfd` as a non-owning borrow. Pass to nix `*at` syscalls via
    /// `as_raw_fd()` when nix's API still uses `RawFd`.
    #[must_use]
    pub fn dirfd(&self) -> BorrowedFd<'_> {
        // SAFETY: `self.dirfd` is an `OwnedFd` so its lifetime is bound to
        // `&self`.
        unsafe { BorrowedFd::borrow_raw(self.dirfd.as_raw_fd()) }
    }

    /// `dirfd` as a raw file descriptor. Lifetime is tied to `&self`.
    #[must_use]
    pub fn dirfd_raw(&self) -> RawFd {
        self.dirfd.as_raw_fd()
    }

    /// `dir.join(name)`, useful when a syscall API requires an owned
    /// `PathBuf` (e.g. `std::fs::remove_dir_all`).
    #[must_use]
    pub fn join(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// Resolve `$HOME` (or platform equivalent). Lifted from the existing
/// `instance::store` helper so the `process` module does not depend on
/// `instance` during the migration.
fn home_dir() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    None
}

/// `mkdir -p <dir>` with the supplied mode (unix only; windows returns
/// `UnsupportedPlatform`).
#[cfg(unix)]
pub(crate) fn ensure_dir_with_mode(dir: &Path, mode: u32) -> Result<(), ProcessError> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(dir)
        .map_err(ProcessError::Io)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_dir_with_mode(_dir: &Path, _mode: u32) -> Result<(), ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

/// `open(dir, O_DIRECTORY | O_CLOEXEC | O_RDONLY)` (unix only).
#[cfg(unix)]
fn open_dirfd(dir: &Path) -> Result<OwnedFd, ProcessError> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)
        .map_err(ProcessError::Io)?;
    Ok(file.into())
}

#[cfg(not(unix))]
fn open_dirfd(_dir: &Path) -> Result<OwnedFd, ProcessError> {
    Err(ProcessError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn create_for_new_id_makes_directory_and_dirfd() {
        let tmp = TempDir::new().expect("tmp");
        let id = ProcessId::generate();
        let paths = ProcPaths::create_for_new_id(tmp.path(), id).expect("create");

        let expected = tmp.path().join(id.to_string());
        assert_eq!(paths.dir(), &expected);
        assert!(expected.exists(), "directory should have been created");
        // dirfd raw integer is non-negative on all sane platforms.
        assert!(paths.dirfd_raw() >= 0);
    }

    #[test]
    fn open_existing_returns_dirfd_for_existing_dir() {
        let tmp = TempDir::new().expect("tmp");
        let id = ProcessId::generate();
        let _created = ProcPaths::create_for_new_id(tmp.path(), id).expect("create");
        let opened = ProcPaths::open_existing(tmp.path(), id).expect("open");
        assert_eq!(opened.id(), id);
    }

    #[cfg(unix)]
    #[test]
    fn directory_permissions_are_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().expect("tmp");
        let id = ProcessId::generate();
        let paths = ProcPaths::create_for_new_id(tmp.path(), id).expect("create");

        let meta = std::fs::metadata(paths.dir()).expect("metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, DIR_MODE);
    }
}
