//! Atomic status file (`<dir>/status`) with a layered locking model.
//!
//! Two locking layers, both required:
//!
//! 1. **Intra-process**: `std::sync::Mutex<File>` inside [`ProcessStatusFile`].
//!    Same-process callers (Runtime + Handle) share an `Arc<ProcessStatusFile>`
//!    so the Mutex is the canonical re-entry barrier on a single OFD.
//! 2. **Cross-process**: `flock(LOCK_EX)` on the file. Independent CLI
//!    invocations (`iter ps`, `iter stop`) open their own fd and serialize
//!    via the kernel's per-inode flock table.
//!
//! All real work runs inside `tokio::task::spawn_blocking` so the synchronous
//! flock hold never crosses an `.await`. Cancellation of the outer
//! `JoinHandle` does not interrupt the closure (Tokio guarantee), so the
//! [`flock::FlockGuard`] `Drop` path always releases the kernel lock.
//!
//! # Layout
//!
//! - [`flock`] — `FlockGuard` + raw `flock(2)` syscalls + the dirfd-vanished
//!   probe.
//! - [`body`] — pure body-level read / write / fsync / rollback helpers.
//! - [`reconcile`] — the rev17 §C3 reconciliation tree consumed by
//!   `Handle::refresh_status`.
//! - [`locked_section`] — the `Initializing → Running` startup / adoption
//!   critical sections (the only paths that may write `Running`).
//! - this `mod.rs` — the public [`ProcessStatusFile`] async surface that
//!   composes the four under the layered locks. The startup/adoption
//!   methods extend `ProcessStatusFile` from
//!   [`locked_section`](self::locked_section).

mod body;
mod flock;
mod locked_section;
mod reconcile;

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::task::spawn_blocking;

use crate::process::error::ProcessError;
use crate::process::paths::{FILE_MODE, ProcPaths, names};
use crate::process::status::{self, ProcessStatus, TransitionResult};

use body::{fsync_with_one_retry, read_status, write_status_in_place};
use flock::FlockGuard;

/// Single-open + Mutex-guarded handle to `<dir>/status`.
///
/// Constructed only by [`ProcessStatusFile::create_initial_locked`] (new
/// session) or [`ProcessStatusFile::open_for_existing`] (CLI tools opening
/// an already-running process). Always shared via `Arc` per the plan's
/// intra-process discipline.
#[derive(Debug)]
pub struct ProcessStatusFile {
    /// Resolved path of the status file (kept for diagnostics; all real
    /// I/O goes through the held `File`).
    path: PathBuf,
    /// The file handle. Mutex is `std::sync` so the lock guard never crosses
    /// `.await` — all critical sections run inside `spawn_blocking`.
    mutex: Mutex<File>,
}

impl ProcessStatusFile {
    /// Create `<paths.dir()>/status` with `O_CREAT|O_EXCL|0600`, write the
    /// initial `initializing` token under flock, and return the wrapper.
    ///
    /// Used by foreground startup (`ProcessSession::create_initial`) and
    /// detached parent registration (`Spawner::register_for_detached`).
    pub async fn create_initial_locked(paths: Arc<ProcPaths>) -> Result<Arc<Self>, ProcessError> {
        spawn_blocking(move || {
            let path = paths.join(names::STATUS);
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .mode(FILE_MODE)
                .open(&path)
                .map_err(ProcessError::Io)?;
            // Write Initializing under flock so other openers see a
            // well-formed body even on first observation.
            let guard = FlockGuard::acquire_exclusive(&file).map_err(ProcessError::FlockAcquire)?;
            let res: Result<(), ProcessError> = (|| {
                write_status_in_place(&mut file, ProcessStatus::Initializing)
                    .map_err(ProcessError::Io)?;
                fsync_with_one_retry(&file).map_err(ProcessError::Io)?;
                Ok(())
            })();
            let release = guard.release();
            match (res, release) {
                (Ok(()), Ok(())) => Ok(Arc::new(Self {
                    path,
                    mutex: Mutex::new(file),
                })),
                (Ok(()), Err(io)) => Err(ProcessError::FlockRelease(io)),
                (Err(e), _) => Err(e),
            }
        })
        .await
        .map_err(|je| map_join_to_process_error(&je))?
    }

    /// Open `<paths.dir()>/status` for an already-running process (used by
    /// CLI tools that read/refresh state). Does not write.
    pub async fn open_for_existing(paths: Arc<ProcPaths>) -> Result<Arc<Self>, ProcessError> {
        spawn_blocking(move || {
            let path = paths.join(names::STATUS);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&path)
                .map_err(ProcessError::Io)?;
            Ok(Arc::new(Self {
                path,
                mutex: Mutex::new(file),
            }))
        })
        .await
        .map_err(|je| map_join_to_process_error(&je))?
    }

    /// Path of the status file (for diagnostics).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Generic transition `from → to` under flock. **Cannot be used to
    /// reach `Running`** — `is_allowed` rejects every `to == Running`,
    /// matching the rev10 invariant that Running is only set inside
    /// `locked_initial_write` / `locked_adoption_write`.
    pub async fn transition(
        self: Arc<Self>,
        from: ProcessStatus,
        to: ProcessStatus,
    ) -> Result<TransitionResult, ProcessError> {
        spawn_blocking(move || {
            self.with_locked_status(|file| -> Result<TransitionResult, ProcessError> {
                let observed = read_status(file).map_err(ProcessError::from)?;
                if observed != from {
                    return Err(ProcessError::IllegalTransition {
                        from,
                        to,
                        observed: Some(observed),
                    });
                }
                if !status::is_allowed(from, to) {
                    return Err(ProcessError::IllegalTransition {
                        from,
                        to,
                        observed: None,
                    });
                }
                write_status_in_place(file, to).map_err(ProcessError::Io)?;
                fsync_with_one_retry(file).map_err(ProcessError::Io)?;
                Ok(TransitionResult { from, to })
            })
        })
        .await
        .map_err(|je| map_join_to_process_error(&je))?
    }

    /// Read the current status under `flock` + `Mutex` (no reconciliation).
    ///
    /// Serialised the same way as [`Self::transition`] and
    /// [`Self::reconcile_under_lock`]. Use this when the caller only needs
    /// to observe the on-disk token; for read+modify+write under the same
    /// critical section, use [`Self::reconcile_under_lock`] instead.
    pub async fn read_status(self: Arc<Self>) -> Result<ProcessStatus, ProcessError> {
        spawn_blocking(move || {
            self.with_locked_status(|file| read_status(file).map_err(ProcessError::from))
        })
        .await
        .map_err(|je| map_join_to_process_error(&je))?
    }

    /// Run the rev17 §C3 reconciliation tree under flock+Mutex. Used by
    /// `Handle::refresh_status` (and `iter ps`).
    ///
    /// The reconciler observes `(status, pid_file_state, grace_elapsed,
    /// alive?)` atomically and writes `Failed` when the on-disk evidence
    /// indicates a crashed/orphaned record. Terminal records are not
    /// transitioned again; cleanup of leftover `bootstrap_token` /
    /// `.pid.tmp` / `linkat` residue runs in the same critical section so a
    /// single refresh tick converges to `nlink == 1`.
    ///
    /// Returns the post-reconcile [`ProcessStatus`]. Environmental I/O errors
    /// (`PermissionDenied`, `SecurityViolation`, transient/fatal `pid_file`
    /// I/O) are surfaced via `tracing::warn!` and leave the on-disk status
    /// untouched until a richer `Diagnostic` plumbing lands.
    pub async fn reconcile_under_lock(
        self: Arc<Self>,
        paths: Arc<ProcPaths>,
        started_at: DateTime<Utc>,
        grace: Duration,
    ) -> Result<ProcessStatus, ProcessError> {
        spawn_blocking(move || {
            self.with_locked_status(|file| {
                reconcile::reconcile_inner(file, paths.dirfd(), started_at, grace)
            })
        })
        .await
        .map_err(|je| map_join_to_process_error(&je))?
    }

    /// The canonical `with_locked_status` helper. **Synchronous**, called from
    /// inside `spawn_blocking`. Generic over outer error type `E` so the same
    /// shape works for `ProcessError` (transition / reconcile),
    /// `StartupError` (`locked_initial_write`), and `AdoptError`
    /// (`locked_adoption_write`).
    pub(super) fn with_locked_status<R, F, E>(&self, f: F) -> Result<R, E>
    where
        F: FnOnce(&mut File) -> Result<R, E>,
        E: From<ProcessError>,
    {
        let mut guard: MutexGuard<'_, File> =
            self.mutex.lock().map_err(|p| E::from(handle_poison(p)))?;
        let flock = FlockGuard::acquire_exclusive(&guard)
            .map_err(|io| E::from(ProcessError::FlockAcquire(io)))?;
        let primary = f(&mut guard);
        let release = flock.release();
        match (primary, release) {
            (Ok(v), Ok(())) => Ok(v),
            (Ok(_), Err(io)) => Err(E::from(ProcessError::FlockRelease(io))),
            (Err(e), Ok(())) => Err(e),
            (Err(e), Err(io)) => {
                tracing::warn!(error = %io, "flock release failed; primary error preserved");
                Err(e)
            }
        }
    }
}

fn handle_poison<T>(_p: PoisonError<T>) -> ProcessError {
    ProcessError::StatusFilePoisoned
}

fn map_join_to_process_error(je: &tokio::task::JoinError) -> ProcessError {
    if je.is_panic() {
        ProcessError::StatusFilePoisoned
    } else {
        ProcessError::Io(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("status_file spawn_blocking cancelled: {je}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::bootstrap_token;
    use crate::process::id::{BootstrapToken, ProcessId};
    use crate::process::paths::ProcPaths;
    use crate::process::pid_file::ProcessIdentity;
    use crate::process::proc_info;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use tempfile::TempDir;

    fn fresh_paths() -> (TempDir, Arc<ProcPaths>) {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let paths = ProcPaths::create_for_new_id(tmp.path(), id).unwrap();
        (tmp, paths)
    }

    #[tokio::test]
    async fn create_initial_writes_initializing_token() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .expect("create");
        let body = std::fs::read_to_string(paths.join(names::STATUS)).unwrap();
        assert_eq!(body, "initializing\n");
        let observed = sf.read_status().await.expect("read");
        assert_eq!(observed, ProcessStatus::Initializing);
    }

    #[tokio::test]
    async fn second_create_initial_returns_already_exists() {
        let (_tmp, paths) = fresh_paths();
        ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let err = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .expect_err("second must fail");
        assert!(matches!(err, ProcessError::Io(_)));
    }

    #[tokio::test]
    async fn transition_running_is_rejected() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Generic transition cannot reach Running.
        let err = sf
            .clone()
            .transition(ProcessStatus::Initializing, ProcessStatus::Running)
            .await
            .expect_err("must reject");
        assert!(matches!(
            err,
            ProcessError::IllegalTransition {
                from: ProcessStatus::Initializing,
                to: ProcessStatus::Running,
                observed: None,
            }
        ));
    }

    #[tokio::test]
    async fn transition_initializing_to_failed_succeeds() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let result = sf
            .clone()
            .transition(ProcessStatus::Initializing, ProcessStatus::Failed)
            .await
            .expect("ok");
        assert_eq!(result.from, ProcessStatus::Initializing);
        assert_eq!(result.to, ProcessStatus::Failed);
        let observed = sf.read_status().await.expect("read");
        assert_eq!(observed, ProcessStatus::Failed);
    }

    // ------- reconcile_under_lock ----------

    #[tokio::test]
    async fn reconcile_initializing_within_grace_is_noop_even_without_pid() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // started_at = now, generous grace → not elapsed.
        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), Utc::now(), Duration::from_secs(60))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Initializing);
        // status file unchanged.
        let body = std::fs::read_to_string(paths.join(names::STATUS)).unwrap();
        assert_eq!(body, "initializing\n");
    }

    #[tokio::test]
    async fn reconcile_initializing_grace_elapsed_no_pid_marks_failed() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // started_at well in the past + 0 grace → elapsed.
        let started = Utc::now() - chrono::Duration::seconds(120);
        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), started, Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Failed);
        let body = std::fs::read_to_string(paths.join(names::STATUS)).unwrap();
        assert_eq!(body, "failed\n");
    }

    #[tokio::test]
    async fn reconcile_initializing_grace_elapsed_with_pid_marks_failed_and_cleans_token() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Drop a synthetic pid file + bootstrap_token to simulate stuck-init.
        // pid_file::read enforces mode 0o600, so write through OpenOptions
        // rather than std::fs::write (which leaves the file world-readable).
        let identity = if cfg!(target_os = "linux") {
            ProcessIdentity {
                pid: crate::process::id::Pid::new(7777),
                start_time: proc_info::ProcessStartTime::LinuxClockTicks(42),
                linux_boot_id: Some("0123456789abcdef-deadbeef".into()),
            }
        } else {
            ProcessIdentity {
                pid: crate::process::id::Pid::new(7777),
                start_time: proc_info::ProcessStartTime::MacosEpochMicros(1_700_000_000_000_000),
                linux_boot_id: None,
            }
        };
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(paths.join(names::PID))
            .unwrap();
        f.write_all(identity.to_pid_line().as_bytes()).unwrap();
        let token = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &token).unwrap();

        let started = Utc::now() - chrono::Duration::seconds(120);
        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), started, Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Failed);
        assert!(!paths.join(names::BOOTSTRAP_TOKEN).exists());
    }

    #[tokio::test]
    async fn reconcile_terminal_status_keeps_status_and_cleans_residue() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Move to terminal Killed via a legal transition.
        sf.clone()
            .transition(ProcessStatus::Initializing, ProcessStatus::Killed)
            .await
            .expect("transition to killed");
        // Drop a stale token that should be swept on the next reconcile.
        let token = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &token).unwrap();
        assert!(paths.join(names::BOOTSTRAP_TOKEN).exists());

        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), Utc::now(), Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Killed);
        assert!(!paths.join(names::BOOTSTRAP_TOKEN).exists());
    }

    #[tokio::test]
    async fn reconcile_running_without_pid_marks_failed() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Hand-write `running\n` to bypass the C1 invariant — this is the
        // exact "should never happen" state we want refresh to repair.
        std::fs::write(paths.join(names::STATUS), b"running\n").unwrap();

        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), Utc::now(), Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Failed);
    }

    #[tokio::test]
    async fn reconcile_running_with_alive_pid_keeps_running() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Use the current process's identity so the aliveness probe
        // returns true.
        let me = proc_info::current_identity().expect("current_identity");
        sf.clone()
            .locked_initial_write(me, paths.clone())
            .await
            .expect("locked_initial_write");

        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), Utc::now(), Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Running);
    }

    #[tokio::test]
    async fn reconcile_corrupt_status_within_grace_returns_corrupt_error() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Truncate to zero bytes → CorruptStatusKind::EmptyBody.
        std::fs::write(paths.join(names::STATUS), b"").unwrap();

        let err = sf
            .clone()
            .reconcile_under_lock(paths.clone(), Utc::now(), Duration::from_secs(30))
            .await
            .expect_err("must surface corrupt status while still in grace");
        assert!(matches!(err, ProcessError::CorruptStatus(_)));
    }

    #[tokio::test]
    async fn reconcile_corrupt_status_after_grace_marks_failed() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        std::fs::write(paths.join(names::STATUS), b"").unwrap();

        let started = Utc::now() - chrono::Duration::seconds(120);
        let observed = sf
            .clone()
            .reconcile_under_lock(paths.clone(), started, Duration::from_secs(30))
            .await
            .expect("reconcile");
        assert_eq!(observed, ProcessStatus::Failed);
        let body = std::fs::read_to_string(paths.join(names::STATUS)).unwrap();
        assert_eq!(body, "failed\n");
    }
}
