//! `ProcessRegistry` — name + id allocation for new process directories.
//!
//! Encapsulates the §I.1 contract:
//!
//! ```text
//! register(name) -> (Arc<ProcessSession>, LockGuard, [BootstrapToken])
//! ```
//!
//! Foreground and detached share the locking + id-allocation + session-init
//! sequence; detached additionally writes the `bootstrap_token`.
//!
//! The registry owns `~/.iter/proc/.locks/`'s dirfd (an `Arc<OwnedFd>`); it
//! is shared into every `spawn_blocking` closure that runs `name_lock::acquire`
//! so the kernel `flock` is always acquired off the Tokio worker thread.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::process::bootstrap_token;
use crate::process::error::{ProcessError, RegistryError};
use crate::process::id::{BootstrapToken, ProcessId};
use crate::process::metadata::ProcessMetadata;
use crate::process::name_lock::{self, LockGuard};
use crate::process::paths::proc_root_default;
use crate::process::record::{ProcessRecord, list_under};
use crate::process::session::ProcessSession;

/// Caller-provided fields for `meta.json`. The `id` and `name` are filled in
/// by [`ProcessRegistry`] at allocation time so callers cannot accidentally
/// disagree with the lock body / directory name.
#[derive(Clone, Debug)]
pub struct MetadataDraft {
    /// Absolute path of the loaded `Iterfile`.
    pub iterfile: PathBuf,
    /// CLI subcommand verb.
    pub subcommand: String,
    /// Wall-clock at session creation.
    pub started_at: DateTime<Utc>,
    /// argv (excluding `argv[0]`).
    pub args: Vec<String>,
    /// Environment overrides applied at spawn time.
    pub env: Vec<(String, String)>,
    /// `--debug` flag.
    pub debug: bool,
    /// Parent process id when this record is being created by an
    /// orchestrator (`iter compose up`); `None` for top-level invocations.
    pub parent_id: Option<ProcessId>,
    /// Free-form labels persisted into `meta.json`. Keys in the
    /// `iter.<feature>.<key>` namespace are reserved for internal use.
    pub labels: BTreeMap<String, String>,
}

impl MetadataDraft {
    fn finalize(self, id: ProcessId, name: String) -> ProcessMetadata {
        ProcessMetadata {
            id,
            name,
            iterfile: self.iterfile,
            subcommand: self.subcommand,
            started_at: self.started_at,
            args: self.args,
            env: self.env,
            debug: self.debug,
            parent_id: self.parent_id,
            labels: self.labels,
        }
    }
}

/// Owner of `~/.iter/proc/` and `~/.iter/proc/.locks/`.
///
/// Cheap to clone-by-`Arc` — every operation re-derives state from the held
/// `proc_root` and the dirfd.
#[derive(Debug)]
pub struct ProcessRegistry {
    proc_root: PathBuf,
    locks_dir_path: PathBuf,
    locks_dirfd: Arc<OwnedFd>,
}

impl ProcessRegistry {
    /// Open the default registry rooted at `~/.iter/proc/`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn open_default() -> Result<Self, ProcessError> {
        let root = proc_root_default()?;
        Self::open(root)
    }

    /// Open a registry rooted at `proc_root` (creates `proc_root/` and
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// `proc_root/.locks/` with mode `0o700` if missing).
    pub fn open(proc_root: impl Into<PathBuf>) -> Result<Self, ProcessError> {
        let proc_root = proc_root.into();
        let (locks_dir_path, locks_dirfd) =
            name_lock::open_locks_dir(&proc_root).map_err(registry_to_process)?;
        Ok(Self {
            proc_root,
            locks_dir_path,
            locks_dirfd: Arc::new(locks_dirfd),
        })
    }

    /// Root directory (`~/.iter/proc/` by default).
    #[must_use]
    pub fn proc_root(&self) -> &Path {
        &self.proc_root
    }

    /// Acquire `<name>` and create a new foreground session.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// On success the returned `LockGuard` and `ProcessSession` should both be
    /// kept alive for the lifetime of the running process; the runtime later
    /// calls `LockGuard::release()` from `Handle::remove()` once the record is
    /// terminal. Dropping the guard early just releases the kernel `flock` —
    /// the on-disk `.locks/<name>` file remains until `stale_check` recovers
    /// it.
    pub async fn register_foreground(
        &self,
        name: &str,
        draft: MetadataDraft,
    ) -> Result<(Arc<ProcessSession>, LockGuard), RegisterError> {
        let id = ProcessId::generate();
        let lock = self.acquire(name, id).await?;
        let session = match self.create_session(name, id, draft).await {
            Ok(s) => s,
            Err(e) => {
                self.cleanup_session_dir(id);
                drop(lock.release());
                return Err(RegisterError::Process(e));
            }
        };
        Ok((session, lock))
    }

    /// Acquire `<name>`, create a new detached session, and publish the
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// bootstrap token at `<dir>/bootstrap_token` (mode `0o600`,
    /// `O_CREAT|O_EXCL`).
    ///
    /// The returned `BootstrapToken` is what the parent advertises — the
    /// child re-reads the same value from disk during adoption.
    pub async fn register_detached(
        &self,
        name: &str,
        draft: MetadataDraft,
    ) -> Result<(Arc<ProcessSession>, LockGuard, BootstrapToken), RegisterError> {
        let id = ProcessId::generate();
        let lock = self.acquire(name, id).await?;
        let session = match self.create_session(name, id, draft).await {
            Ok(s) => s,
            Err(e) => {
                self.cleanup_session_dir(id);
                drop(lock.release());
                return Err(RegisterError::Process(e));
            }
        };
        let token = BootstrapToken::generate();
        if let Err(io) = bootstrap_token::write_excl(session.paths().dirfd(), &token) {
            // Half-init cleanup: drop the partial dir and release the lock so
            // the name is reusable.
            drop(session);
            self.cleanup_session_dir(id);
            drop(lock.release());
            return Err(RegisterError::Process(ProcessError::Io(io)));
        }
        Ok((session, lock, token))
    }

    /// Enumerate every record under `proc_root` (skips `.locks/` and any
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// other dot-directory).
    pub fn list(&self) -> Result<Vec<ProcessRecord>, ProcessError> {
        list_under(&self.proc_root)
    }

    /// Look up a record by id.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn get(&self, id: ProcessId) -> Result<ProcessRecord, ProcessError> {
        ProcessRecord::open(&self.proc_root, id)
    }

    /// Look up a record by registered name (linear scan over `list`).
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// Returns `Ok(None)` if no record matches.
    pub fn find_by_name(&self, name: &str) -> Result<Option<ProcessRecord>, ProcessError> {
        for rec in self.list()? {
            if let Ok(n) = rec.name()
                && n == name
            {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    // -----------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------

    /// Run `name_lock::acquire` inside `spawn_blocking` so the held flock
    /// never crosses an `.await`.
    async fn acquire(&self, name: &str, id: ProcessId) -> Result<LockGuard, RegisterError> {
        let proc_root = self.proc_root.clone();
        let locks_dir = self.locks_dir_path.clone();
        let dirfd = Arc::clone(&self.locks_dirfd);
        let name = name.to_owned();
        let join = tokio::task::spawn_blocking(move || {
            // The Arc keeps the OwnedFd alive for the duration of the
            // closure; `as_fd()` borrows from it.
            let borrowed = dirfd.as_fd();
            name_lock::acquire(borrowed, &locks_dir, &proc_root, &name, id)
        })
        .await;
        match join {
            Ok(inner) => inner.map_err(RegisterError::Registry),
            Err(je) if je.is_panic() => Err(RegisterError::Registry(RegistryError::Io(
                io::Error::other("name_lock acquire task panicked"),
            ))),
            Err(_) => Err(RegisterError::Registry(RegistryError::Io(
                io::Error::other("name_lock acquire task cancelled"),
            ))),
        }
    }

    async fn create_session(
        &self,
        name: &str,
        id: ProcessId,
        draft: MetadataDraft,
    ) -> Result<Arc<ProcessSession>, ProcessError> {
        let metadata = draft.finalize(id, name.to_owned());
        ProcessSession::create_initial(&self.proc_root, metadata).await
    }

    fn cleanup_session_dir(&self, id: ProcessId) {
        let dir = self.proc_root.join(id.to_string());
        drop(std::fs::remove_dir_all(&dir));
    }
}

/// Map a [`RegistryError`] into a [`ProcessError`] for the `open` /
/// `open_default` paths, which only deal with environment-level setup.
fn registry_to_process(e: RegistryError) -> ProcessError {
    match e {
        RegistryError::Io(io) => ProcessError::Io(io),
        other => ProcessError::Io(io::Error::other(other.to_string())),
    }
}

/// Outer error returned by [`ProcessRegistry::register_foreground`] /
/// [`ProcessRegistry::register_detached`].
///
/// The two arms distinguish "name lock contention or filesystem unsupported"
/// from "session directory I/O failure" so the CLI can render `AlreadyExists`
/// as a friendly user message and bubble the rest as diagnostics.
#[derive(Debug)]
#[non_exhaustive]
pub enum RegisterError {
    /// Failure inside the name-lock acquisition path.
    Registry(RegistryError),
    /// Failure inside session creation or bootstrap-token publication.
    Process(ProcessError),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::Registry(e) => write!(f, "name lock: {e}"),
            RegisterError::Process(e) => write!(f, "process directory: {e}"),
        }
    }
}

impl std::error::Error for RegisterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegisterError::Registry(e) => Some(e),
            RegisterError::Process(e) => Some(e),
        }
    }
}

impl From<RegisterError> for ProcessError {
    fn from(e: RegisterError) -> Self {
        match e {
            RegisterError::Process(p) => p,
            RegisterError::Registry(r) => registry_to_process(r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::bootstrap_token;
    use crate::process::status::ProcessStatus;
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

    fn sample_draft_detached() -> MetadataDraft {
        sample_draft()
    }

    #[tokio::test]
    async fn open_creates_proc_root_and_locks_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proc");
        let _registry = ProcessRegistry::open(&root).expect("open");
        assert!(root.exists());
        assert!(root.join(".locks").exists());
    }

    #[tokio::test]
    async fn register_foreground_writes_initializing_status_and_metadata() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");

        assert_eq!(session.name(), "alpha");
        assert_eq!(
            session
                .status_file()
                .read_status()
                .await
                .expect("read status"),
            ProcessStatus::Initializing
        );
        // meta.json round-trips via ProcessRecord.
        let rec = registry.get(session.id()).expect("get");
        let meta = rec.metadata().expect("meta");
        assert_eq!(meta.id, session.id());
        assert_eq!(meta.name, "alpha");
        // Lock body is published.
        assert!(tmp.path().join(".locks").join("alpha").exists());

        drop(lock.release());
    }

    #[tokio::test]
    async fn register_foreground_rejects_duplicate_when_existing_record_is_live() {
        // Plant a stale lock body whose body points at a fake "running" record.
        // Mirrors the `name_lock::acquire_rejects_when_record_is_live` pattern:
        // by simulating a holder that has already released its flock but whose
        // record is still active, we exercise the AlreadyExists branch without
        // single-process flock contention.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");

        let owner = ProcessId::generate();
        let owner_dir = tmp.path().join(owner.to_string());
        std::fs::create_dir_all(&owner_dir).unwrap();
        std::fs::set_permissions(&owner_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(owner_dir.join("status"), b"running\n").unwrap();
        let lock_path = tmp.path().join(".locks").join("alpha");
        std::fs::write(
            &lock_path,
            format!("{}\n{}\n", owner, Utc::now().to_rfc3339()),
        )
        .unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let err = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect_err("second must fail");
        match err {
            RegisterError::Registry(RegistryError::AlreadyExists) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_foreground_rejects_invalid_name() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let err = registry
            .register_foreground("bad/name", sample_draft())
            .await
            .expect_err("must fail");
        match err {
            RegisterError::Registry(RegistryError::InvalidName { .. }) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_detached_publishes_bootstrap_token() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, _lock, token) = registry
            .register_detached("beta", sample_draft_detached())
            .await
            .expect("register");

        // Token round-trips via the on-disk file.
        let on_disk = bootstrap_token::read(session.paths().dirfd()).expect("read token");
        assert_eq!(on_disk, token);
    }

    #[tokio::test]
    async fn lock_release_frees_the_name_for_reuse() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session1, lock1) = registry
            .register_foreground("gamma", sample_draft())
            .await
            .expect("first");
        let id1 = session1.id();
        // Drop the session and explicitly release the lock + remove dir to
        // simulate a finished record. (`stale_check` would otherwise keep us
        // out: the status is still Initializing.)
        drop(session1);
        std::fs::remove_dir_all(tmp.path().join(id1.to_string())).expect("rm dir");
        lock1.release().expect("release");

        let (session2, _lock2) = registry
            .register_foreground("gamma", sample_draft())
            .await
            .expect("second");
        assert_ne!(session2.id(), id1);
    }

    #[tokio::test]
    async fn list_and_find_by_name_round_trip() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (s1, _l1) = registry
            .register_foreground("one", sample_draft())
            .await
            .unwrap();
        let (s2, _l2) = registry
            .register_foreground("two", sample_draft())
            .await
            .unwrap();

        let listed = registry.list().expect("list");
        assert_eq!(listed.len(), 2);

        let found = registry
            .find_by_name("two")
            .expect("find")
            .expect("present");
        assert_eq!(found.id(), s2.id());
        assert_ne!(found.id(), s1.id());
        assert!(registry.find_by_name("missing").expect("find").is_none());
    }
}
