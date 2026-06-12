//! `ProcessHandle` — read + control surface over a single proc directory.
//!
//! `ProcessHandle` is the only way for the rest of the codebase (CLI,
//! `iter ps`, runner finalize) to *change* the on-disk state of a process
//! record. Every status mutation goes through one of the
//! `process::status_file` primitives — `transition`, `reconcile_under_lock`,
//! or the locked-startup writers — so the rev17 §B4 invariant ("all status
//! writes are routed through the `status_file` locking primitives") is
//! preserved by construction.
//!
//! # Two construction paths
//!
//! - [`ProcessHandle::from_session`] is the *intra-process* constructor. It
//!   shares the session's `Arc<ProcessStatusFile>`, which is what makes the
//!   intra-process `Mutex<File>` actually serialise concurrent calls (rev17
//!   §B2). The runner uses this when it builds the `ProcessHandle` it hands
//!   back to `Handle::stop` invocations originating from the same OS process.
//!
//! - [`ProcessHandle::open`] is the *cross-process* constructor — used by
//!   `iter ps` / `iter stop` invoked from a separate CLI process. It opens
//!   its own status fd via [`ProcessStatusFile::open_for_existing`], which
//!   has its own intra-process `Mutex<File>`. Cross-process serialisation is
//!   provided exclusively by `flock(2)` inside the locked critical sections.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::process::error::{ProcessError, Result};
use crate::process::id::ProcessId;
use crate::process::paths::ProcPaths;
#[cfg(unix)]
use crate::process::pid_file::PidFileState;
use crate::process::proc_info::process_is_alive_with_start_time;
use crate::process::record::ProcessRecord;
use crate::process::session::ProcessSession;
use crate::process::posix_signal::{self, PosixSignal};
use crate::process::status::{ProcessStatus, TransitionResult};
use crate::process::status_file::ProcessStatusFile;

/// Default bootstrap grace window (rev17 §C3): how long an `Initializing`
/// record without a pid file is given before `reconcile_under_lock` upgrades
/// it to `Failed`.
const DEFAULT_BOOTSTRAP_GRACE_SECS: u64 = 30;

/// Environment variable that overrides [`DEFAULT_BOOTSTRAP_GRACE_SECS`].
pub const BOOTSTRAP_GRACE_ENV: &str = "ITER_PROCESS_BOOTSTRAP_GRACE_SECS";

/// Resolve the bootstrap grace window. Reads
/// `ITER_PROCESS_BOOTSTRAP_GRACE_SECS` on every call (no caching, since
/// `refresh_status` runs at most a few hertz); falls back to 30 seconds
/// when unset or unparseable.
#[must_use]
pub fn bootstrap_grace() -> Duration {
    match std::env::var(BOOTSTRAP_GRACE_ENV) {
        Ok(s) => match s.trim().parse::<u64>() {
            Ok(n) => Duration::from_secs(n),
            Err(_) => Duration::from_secs(DEFAULT_BOOTSTRAP_GRACE_SECS),
        },
        Err(_) => Duration::from_secs(DEFAULT_BOOTSTRAP_GRACE_SECS),
    }
}

/// Read + control surface over `~/.iter/proc/<id>/`.
#[derive(Debug, Clone)]
pub struct ProcessHandle {
    record: ProcessRecord,
    status_file: Arc<ProcessStatusFile>,
}

impl ProcessHandle {
    /// Cross-process constructor. Opens the existing proc directory at
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// `<root>/<id>/` and instantiates a new status fd via
    /// [`ProcessStatusFile::open_for_existing`].
    pub async fn open(root: &Path, id: ProcessId) -> Result<Self> {
        let record = ProcessRecord::open(root, id)?;
        let status_file = ProcessStatusFile::open_for_existing(record.paths().clone()).await?;
        Ok(Self {
            record,
            status_file,
        })
    }

    /// Intra-process constructor. Shares the session's `Arc<ProcessStatusFile>`
    /// so the runner and any handle observed concurrently serialise over the
    /// same intra-process `Mutex<File>` (rev17 §B2).
    #[must_use]
    pub fn from_session(session: &Arc<ProcessSession>) -> Self {
        let paths: Arc<ProcPaths> = session.paths();
        let status_file = session.status_file();
        Self {
            record: ProcessRecord::new(paths),
            status_file,
        }
    }

    /// `ProcessId` of the underlying record.
    #[must_use]
    pub fn id(&self) -> ProcessId {
        self.record.id()
    }

    /// Borrow the underlying [`ProcessRecord`] for read-only accessors.
    #[must_use]
    pub fn record(&self) -> &ProcessRecord {
        &self.record
    }

    /// Process directory paths.
    #[must_use]
    pub fn paths(&self) -> &Arc<ProcPaths> {
        self.record.paths()
    }

    /// Read the current status under `flock` + `Mutex` (no reconciliation).
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn status(&self) -> Result<ProcessStatus> {
        self.status_file.clone().read_status().await
    }

    /// Run the rev17 §C3 reconciliation tree. Reads `started_at` from the
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// side file and feeds it (together with [`bootstrap_grace`]) to
    /// [`ProcessStatusFile::reconcile_under_lock`].
    pub async fn refresh_status(&self) -> Result<ProcessStatus> {
        let started_at = self.record.started_at()?;
        self.status_file
            .clone()
            .reconcile_under_lock(self.record.paths().clone(), started_at, bootstrap_grace())
            .await
    }

    /// Send `SIGTERM` to the recorded pid (best-effort) and mark the record
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// `Killed`.
    ///
    /// Tries `transition(Running → Killed)` first; if the on-disk status was
    /// still `Initializing`, retries with `transition(Initializing → Killed)`.
    /// Already-terminal records are surfaced via
    /// [`ProcessError::IllegalTransition`] so the caller can render
    /// "already stopped".
    pub async fn stop(&self) -> Result<TransitionResult> {
        self.signal_and_kill(PosixSignal::Term).await
    }

    /// `SIGKILL` analogue of [`Self::stop`].
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn kill(&self) -> Result<TransitionResult> {
        self.signal_and_kill(PosixSignal::Kill).await
    }

    /// Force-deliver `SIGKILL` to a process whose record is already terminal
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// but whose pid is still alive (matching `start_time`).
    ///
    /// This exists because [`ProcessStatus::is_terminal`] reflects user
    /// *intent* (`stop` flips the record to `Killed` synchronously), not
    /// kernel exit. A process stuck in a non-cancellable hook after `iter
    /// stop` keeps the same pid until it eventually exits, and `iter kill`
    /// must still be able to escalate.
    ///
    /// Returns `Ok(true)` when SIGKILL was delivered, `Ok(false)` when
    /// the recorded pid file is missing or the kernel reports the process
    /// gone (so there is nothing to escalate). Returns `Err` when the pid
    /// file exists but is unreadable for an environmental reason
    /// (`PermissionDenied`, `SecurityViolation`, `IoTransient`,
    /// `IoFatal`, `Corrupt`) — those must surface so the caller never
    /// silently reports "nothing to kill" while a live process remains
    /// (Codex iter-13 Minor C3). Does *not* attempt a status transition
    /// — the caller is expected to use this only when the status is
    /// already terminal, or when status reconciliation itself failed
    /// and the pid-file path is the only escalation route left.
    #[cfg(unix)]
    pub fn force_kill(&self) -> Result<bool> {
        // Bounded retry on EINTR/EAGAIN: `IoTransient` is documented as
        // "retryable" by `pid_file::read`, and the only caller of
        // `force_kill` (the SIGKILL escalation pass) does not retry per
        // service. Without this loop a single signal-interrupted open(2)
        // would leave a live process unkilled. (Codex iter-14 Minor D2)
        const MAX_TRANSIENT_RETRIES: u32 = 3;
        let mut transient_attempt = 0;
        let identity = loop {
            match self.record.pid_identity() {
                PidFileState::Found(id) => break id,
                PidFileState::NotFound => return Ok(false),
                PidFileState::Corrupt(kind) => {
                    return Err(ProcessError::CorruptPidFile {
                        raw_bytes: Vec::new(),
                        reason: format!("{kind:?}"),
                    });
                }
                PidFileState::PermissionDenied => {
                    return Err(ProcessError::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "pid file unreadable: permission denied",
                    )));
                }
                PidFileState::SecurityViolation(kind) => {
                    return Err(ProcessError::Io(std::io::Error::other(format!(
                        "pid file rejected by security check: {kind:?}"
                    ))));
                }
                PidFileState::IoTransient(err) => {
                    transient_attempt += 1;
                    if transient_attempt >= MAX_TRANSIENT_RETRIES {
                        return Err(ProcessError::Io(err));
                    }
                    // Loop iterates and re-reads pid_identity().
                }
                PidFileState::IoFatal(err) => {
                    return Err(ProcessError::Io(err));
                }
            }
        };
        // Probe failures must surface — silently returning `false` would
        // make `iter kill` report "already gone" and skip the only
        // forceful-escalation path the operator has.
        if !process_is_alive_with_start_time(&identity)? {
            return Ok(false);
        }
        posix_signal::send(identity.pid.as_raw(), PosixSignal::Kill)?;
        Ok(true)
    }

    #[cfg(not(unix))]
    pub fn force_kill(&self) -> Result<bool> {
        Ok(false)
    }

    /// Remove the proc directory and its `.locks/<name>` entry.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// Refuses to run unless the on-disk status is terminal (`Stopped` /
    /// `Failed` / `Killed`); the caller is expected to `stop` / `kill` first
    /// or wait for `refresh_status` to upgrade a stale `Initializing` /
    /// `Running` record.
    ///
    /// A still-live record returns [`ProcessError::NotTerminal`] (not
    /// `IllegalTransition`, which is reserved for state-machine edges that
    /// were attempted and rejected).
    pub async fn remove(&self) -> Result<()> {
        let status = self.status_file.clone().read_status().await?;
        if !status.is_terminal() {
            return Err(ProcessError::NotTerminal { current: status });
        }
        let name = self.record.name().ok();
        let dir = self.record.dir().to_owned();
        let proc_root = match dir.parent() {
            Some(p) => p.to_owned(),
            None => {
                return Err(ProcessError::Io(std::io::Error::other(
                    "proc directory has no parent",
                )));
            }
        };
        let id = self.record.id();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(name) = name.as_deref() {
                release_lock_entry(&proc_root, name, id)?;
            }
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(ProcessError::Io(e)),
            }
        })
        .await
        .map_err(|je| {
            ProcessError::Io(std::io::Error::other(if je.is_panic() {
                "remove task panicked"
            } else {
                "remove task cancelled"
            }))
        })??;
        Ok(())
    }

    async fn signal_and_kill(&self, kind: PosixSignal) -> Result<TransitionResult> {
        // Best-effort signal — an absent / unreadable pid file is not a
        // hard failure (the record may already be terminal, or still in the
        // bootstrap window before pid is published). Errors from the actual
        // signalling are surfaced; the status transition is attempted in any
        // case so the caller's record-level intent is recorded.
        if let Some(pid) = current_pid(&self.record) {
            posix_signal::send(pid, kind)?;
        }
        let sf = self.status_file.clone();
        match sf
            .clone()
            .transition(ProcessStatus::Running, ProcessStatus::Killed)
            .await
        {
            Ok(r) => Ok(r),
            Err(ProcessError::IllegalTransition {
                observed: Some(ProcessStatus::Initializing),
                ..
            }) => {
                sf.transition(ProcessStatus::Initializing, ProcessStatus::Killed)
                    .await
            }
            Err(e) => Err(e),
        }
    }
}

/// Resolve the recorded `pid` into a target for [`posix_signal::send`], or `None`
/// when the pid file is absent / corrupt / non-unix. The signal path is
/// best-effort, so a missing pid is never an error.
#[cfg(unix)]
fn current_pid(record: &ProcessRecord) -> Option<u32> {
    match record.pid_identity() {
        PidFileState::Found(identity) => Some(identity.pid.as_raw()),
        _ => None,
    }
}

#[cfg(not(unix))]
fn current_pid(_record: &ProcessRecord) -> Option<u32> {
    None
}

/// Best-effort release of `<proc_root>/.locks/<name>` when its body matches
/// `expected_id`. Pure delegation — the post-flock `(st_dev, st_ino)`
/// re-validation, bounded body read, and unlinkat live in
/// [`crate::process::name_lock::release_by_id`] so this module owns *no*
/// raw libc state of its own.
fn release_lock_entry(proc_root: &Path, name: &str, expected_id: ProcessId) -> Result<()> {
    use crate::process::name_lock;
    use std::os::fd::AsFd;

    let (_locks_path, locks_dirfd) = name_lock::open_locks_dir(proc_root).map_err(|e| {
        ProcessError::Io(std::io::Error::other(format!(
            "open .locks/ for release: {e}"
        )))
    })?;
    name_lock::release_by_id(locks_dirfd.as_fd(), name, expected_id)
        .map_err(|e| ProcessError::Io(std::io::Error::other(format!("release .locks/{name}: {e}"))))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::process::registry::{MetadataDraft, ProcessRegistry};
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sample_draft() -> MetadataDraft {
        MetadataDraft {
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into()],
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn from_session_shares_status_file_arc() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let handle = ProcessHandle::from_session(&session);
        // Same Arc instance => intra-process Mutex<File> is shared.
        assert!(Arc::ptr_eq(&session.status_file(), &handle.status_file));
        assert_eq!(handle.id(), session.id());
    }

    #[tokio::test]
    async fn open_round_trips_status_initializing() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let handle = ProcessHandle::open(tmp.path(), session.id())
            .await
            .expect("open");
        assert_eq!(
            handle.status().await.expect("status"),
            ProcessStatus::Initializing
        );
    }

    #[tokio::test]
    async fn refresh_status_upgrades_initializing_to_failed_after_grace() {
        // Force a 0-second grace so the reconcile tree treats the bootstrap
        // window as already elapsed.
        // SAFETY: env-var mutation is process-global; tests run serially.
        unsafe {
            std::env::set_var(BOOTSTRAP_GRACE_ENV, "0");
        }

        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let id = session.id();
        // Drop the session so its Arc<ProcessStatusFile> goes away — refresh
        // through a freshly-opened handle to mimic `iter ps` from a separate
        // CLI invocation.
        drop(session);

        let handle = ProcessHandle::open(tmp.path(), id).await.expect("open");
        let observed = handle.refresh_status().await.expect("refresh");
        // The pid file was never published, so `Initializing + NotFound +
        // grace_elapsed` must converge to Failed (rev17 §C3).
        assert_eq!(observed, ProcessStatus::Failed);
    }

    #[tokio::test]
    async fn stop_marks_initializing_record_killed() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let handle = ProcessHandle::from_session(&session);
        let result = handle.stop().await.expect("stop");
        // No pid file, so `Running → Killed` fails IllegalTransition and the
        // fallback `Initializing → Killed` writes the terminal state.
        assert_eq!(result.from, ProcessStatus::Initializing);
        assert_eq!(result.to, ProcessStatus::Killed);
        assert_eq!(
            handle.status().await.expect("status"),
            ProcessStatus::Killed
        );
    }

    #[tokio::test]
    async fn remove_refuses_non_terminal_record() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let handle = ProcessHandle::from_session(&session);
        let err = handle.remove().await.expect_err("must refuse");
        match err {
            ProcessError::NotTerminal {
                current: ProcessStatus::Initializing,
            } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_drops_proc_dir_and_lock_after_terminal() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        let handle = ProcessHandle::from_session(&session);
        // Move to terminal so `remove` accepts.
        handle.stop().await.expect("stop -> Killed");
        let id = handle.id();
        let proc_dir = tmp.path().join(id.to_string());
        let lock_path = tmp.path().join(".locks").join("alpha");
        assert!(proc_dir.exists());
        assert!(lock_path.exists());

        // Drop the runner-side LockGuard (releases its `flock` but leaves the
        // on-disk entry behind, mimicking a runner that exited without
        // calling `LockGuard::release`). Drop the session too so its
        // `Arc<ProcessStatusFile>` goes away.
        drop(lock);
        drop(session);

        handle.remove().await.expect("remove");
        assert!(!proc_dir.exists(), "proc dir must be removed");
        assert!(!lock_path.exists(), "lock entry must be removed");
    }
}
