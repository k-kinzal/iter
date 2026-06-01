//! `adoption` — the child-side entry point for `iter run --process-id <ULID>`
//! (rev17 §C5).
//!
//! When the detached parent has already created `~/.iter/proc/<id>/`, written
//! `meta.json` + `status=initializing` + `bootstrap_token`, and exec'd into
//! the child, the child re-enters the normal `iter run` path and calls
//! [`adopt_from_argv`] to take ownership of the existing record. The actual
//! `Initializing → Running` flip plus pid-file publication happens inside
//! [`crate::process::status_file::ProcessStatusFile::locked_adoption_write`]
//! under flock; this module only orchestrates the prep work:
//!
//! 1. Re-open the directory via [`ProcessSession::adopt`] (validates that
//!    `meta.json::id` matches the argv id).
//! 2. Collect the running process identity (pid + `start_time`, plus `boot_id`
//!    on Linux).
//! 3. Read `bootstrap_token` from disk so [`locked_adoption_write`] has a
//!    value to compare against the in-flock re-read (TOCTOU defense per
//!    rev17 §D3).
//! 4. Run `locked_adoption_write` to publish pid and flip status atomically.
//!
//! Token-level failures (file missing, corrupt body, I/O) collapse into
//! the matching [`AdoptError`] variant before the locked critical section
//! runs; the `locked_adoption_write` re-read is the second layer that
//! guards against concurrent overwrite.
//!
//! [`locked_adoption_write`]: crate::process::status_file::ProcessStatusFile::locked_adoption_write

use std::path::Path;
use std::sync::Arc;

use tracing::warn;

use crate::process::bootstrap_token::{self, TokenReadError};
use crate::process::error::{AdoptError, ProcessError};
use crate::process::id::ProcessId;
use crate::process::paths::ProcPaths;
use crate::process::proc_info::current_identity;
use crate::process::session::ProcessSession;
use crate::process::status::ProcessStatus;
use crate::process::status_file::ProcessStatusFile;

/// Adopt the process record at `<root>/<id>/`, publishing this OS process's
/// # Errors
///
/// Returns an error if the operation fails.
/// pid file and flipping status to `Running` in a single flock-protected
/// critical section.
///
/// On success the returned [`ProcessSession`] is ready to drive a runner
/// (its status fd is shared via `Arc`, the runtime should pull
/// `session.status_file()` for finalize).
///
/// Any failure after the proc directory and status file are open — that
/// includes the metadata read inside [`ProcessSession::adopt`], identity
/// collection, token read, and the locked adoption write — triggers a
/// best-effort `Initializing → Failed` transition before the error
/// propagates so the parent-allocated record does not dangle in
/// `Initializing` until bootstrap-grace expires. The transition is
/// precondition-checked on the observed status, so it is a no-op when
/// a concurrent adopter already flipped the record (e.g. the
/// `AdoptError::AlreadyAdopted` case).
pub async fn adopt_from_argv(
    root: &Path,
    id: ProcessId,
) -> Result<Arc<ProcessSession>, AdoptError> {
    // Open the directory + status file ourselves so the cleanup path
    // always has a status fd, even when `ProcessSession::adopt`'s later
    // steps (`read_metadata`, the meta.json id check) fail.
    let paths = ProcPaths::open_existing(root, id).map_err(AdoptError::from)?;
    let status_file = ProcessStatusFile::open_for_existing(paths.clone())
        .await
        .map_err(AdoptError::from)?;

    match try_adopt(root, id, &status_file).await {
        Ok(session) => Ok(session),
        Err(err) => {
            mark_failed_best_effort(&status_file).await;
            Err(err)
        }
    }
}

async fn try_adopt(
    root: &Path,
    id: ProcessId,
    _cleanup_status_file: &Arc<ProcessStatusFile>,
) -> Result<Arc<ProcessSession>, AdoptError> {
    // Re-enter `ProcessSession::adopt` to construct the full session
    // (metadata id check etc.). Any failure here — including
    // `MetadataIdMismatch` and `read_metadata` I/O — falls into the
    // outer cleanup branch via `?`, where the upfront-opened
    // `_cleanup_status_file` is used to write `Initializing → Failed`.
    let session = ProcessSession::adopt(root, id)
        .await
        .map_err(AdoptError::from)?;

    let identity = current_identity().map_err(AdoptError::from)?;

    let expected = match bootstrap_token::read(session.paths().dirfd()) {
        Ok(t) => t,
        Err(TokenReadError::NotFound) => return Err(AdoptError::AlreadyAdopted),
        Err(TokenReadError::Corrupt(k)) => return Err(AdoptError::CorruptToken(k)),
        Err(TokenReadError::Io(e)) => return Err(ProcessError::Io(e).into()),
    };

    session
        .status_file()
        .locked_adoption_write(identity, session.paths(), expected)
        .await?;

    Ok(session)
}

async fn mark_failed_best_effort(status_file: &Arc<ProcessStatusFile>) {
    if let Err(transition_err) = status_file
        .clone()
        .transition(ProcessStatus::Initializing, ProcessStatus::Failed)
        .await
    {
        // The transition is precondition-checked, so a record that already
        // moved past `Initializing` (e.g. the `AlreadyAdopted` case where a
        // concurrent adopter flipped to `Running`) lands here as
        // `IllegalTransition`. That is the expected no-op result — only
        // log it so the typed primary error reaches the caller unmasked.
        warn!(
            error = %transition_err,
            "best-effort Initializing→Failed transition failed during adopt cleanup",
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::process::registry::{MetadataDraft, ProcessRegistry};
    use crate::process::status::ProcessStatus;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn detached_draft() -> MetadataDraft {
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
    async fn adopt_from_argv_publishes_pid_and_flips_to_running() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let (session, lock, _token) = registry
            .register_detached("alpha", detached_draft())
            .await
            .expect("register");
        let id = session.id();
        // Simulate the parent exiting after exec: the parent-side session
        // and lock guard go out of scope without `release` (lock body
        // remains as the on-disk name registry entry).
        drop(session);
        drop(lock);

        let adopted = adopt_from_argv(tmp.path(), id)
            .await
            .expect("adopt_from_argv");

        // Status file is now Running.
        let status = adopted
            .status_file()
            .read_status()
            .await
            .expect("read status");
        assert_eq!(status, ProcessStatus::Running);

        // pid file was published; bootstrap_token was deleted.
        let dir = tmp.path().join(id.to_string());
        assert!(dir.join("pid").exists(), "pid file must exist");
        assert!(
            !dir.join("bootstrap_token").exists(),
            "bootstrap_token must be deleted after adoption"
        );
    }

    #[tokio::test]
    async fn adopt_from_argv_returns_already_adopted_when_token_missing() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let (session, lock, _token) = registry
            .register_detached("alpha", detached_draft())
            .await
            .expect("register");
        let id = session.id();
        drop(session);
        drop(lock);

        // Pretend a concurrent adoption already deleted the token.
        let token_path = tmp.path().join(id.to_string()).join("bootstrap_token");
        std::fs::remove_file(&token_path).expect("remove token");

        let err = adopt_from_argv(tmp.path(), id)
            .await
            .expect_err("must fail");
        match err {
            AdoptError::AlreadyAdopted => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn adopt_from_argv_fails_when_directory_missing() {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let err = adopt_from_argv(tmp.path(), id)
            .await
            .expect_err("must fail");
        match err {
            AdoptError::LockedSection(crate::process::error::LockedSectionError::Io(_)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
