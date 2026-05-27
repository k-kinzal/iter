//! Startup / adoption critical-section logic — the only paths that flip
//! `<dir>/status` from `Initializing → Running`.
//!
//! `mod.rs` owns the generic [`ProcessStatusFile`] surface (constructors,
//! `transition`, `read_status`, `reconcile_under_lock`).
//! This module owns the three startup-flavoured operations plus the
//! shared rollback / spawn_blocking-finalize helpers they all use:
//!
//! - [`ProcessStatusFile::locked_initial_write`] — foreground startup
//! - [`ProcessStatusFile::locked_adoption_write`] — detached adoption
//! - [`publish_running_under_flock`] — the shared post-precheck publish
//! - [`corrupt_to_locked`] — corrupt-body rollback projection
//! - [`finalize_join`] — `spawn_blocking` `JoinError` → outer-error mapping
//!
//! All sub-routines run under the same flock+Mutex critical section.
//! That primitive — [`ProcessStatusFile::with_locked_status`] — is
//! `pub(super)`, deliberately exposed across the `status_file` module so
//! the startup/adoption flows in this file compose with the generic
//! `transition` / `read_status` / `reconcile_under_lock` flows in `mod.rs`
//! without each layer re-implementing flock+Mutex acquisition.

use std::fs::File;
use std::sync::Arc;

use tokio::task::spawn_blocking;

use crate::process::bootstrap_token;
use crate::process::error::{AdoptError, LockedSectionError, ProcessError, StartupError};
use crate::process::id::BootstrapToken;
use crate::process::paths::{ProcPaths, names};
use crate::process::pid_file::{self, ProcessIdentity, PublishError};
use crate::process::status::{CorruptStatusError, ProcessStatus};

use super::ProcessStatusFile;
use super::body::{
    best_effort_mark_failed, fsync_with_one_retry, read_status, write_status_in_place,
};
use super::flock::proc_dir_vanished;

impl ProcessStatusFile {
    /// Foreground startup atomic critical section.
    ///
    /// Sequence (under one flock):
    /// 1. Read status; only `Initializing` is allowed.
    /// 2. Verify the proc dir is still alive (`proc_dir_vanished`).
    /// 3. Publish pid file via `pid_file::write_atomic_at`.
    /// 4. In-place rewrite status to `Running`.
    /// 5. fsync(status) with one retry; on failure rollback to `Failed`.
    pub async fn locked_initial_write(
        self: Arc<Self>,
        identity: ProcessIdentity,
        paths: Arc<ProcPaths>,
    ) -> Result<(), StartupError> {
        let join = spawn_blocking(move || {
            self.with_locked_status(|file| -> Result<(), StartupError> {
                match read_status(file) {
                    Ok(ProcessStatus::Initializing) => {}
                    Ok(ProcessStatus::Killed) => return Err(StartupError::CancelledBeforeStart),
                    Ok(ProcessStatus::Failed) => return Err(StartupError::AlreadyMarkedFailed),
                    Ok(other) => return Err(LockedSectionError::UnexpectedStatus(other).into()),
                    Err(corrupt) => return Err(corrupt_to_locked(file, corrupt).into()),
                }
                publish_running_under_flock(file, &paths, &identity)?;
                Ok(())
            })
        })
        .await;
        finalize_join(join)
    }

    /// Detached adoption atomic critical section. Mirrors
    /// `locked_initial_write` shape; in addition validates the bootstrap
    /// token (file-based, see `process::bootstrap_token`) and best-effort
    /// deletes it after the Running flip.
    pub async fn locked_adoption_write(
        self: Arc<Self>,
        identity: ProcessIdentity,
        paths: Arc<ProcPaths>,
        expected_token: BootstrapToken,
    ) -> Result<(), AdoptError> {
        let join = spawn_blocking(move || {
            self.with_locked_status(|file| -> Result<(), AdoptError> {
                match read_status(file) {
                    Ok(ProcessStatus::Initializing) => {}
                    // Another adopter already won the race: status flipped
                    // to Running and (typically) the bootstrap_token was
                    // unlinked. Map directly to `AlreadyAdopted` so the
                    // domain meaning is preserved instead of falling
                    // through to the `UnexpectedStatus` catch-all.
                    Ok(ProcessStatus::Running) => return Err(AdoptError::AlreadyAdopted),
                    Ok(ProcessStatus::Killed | ProcessStatus::Stopped | ProcessStatus::Failed) => {
                        return Err(AdoptError::ProcessAlreadyTerminated);
                    }
                    Err(corrupt) => return Err(corrupt_to_locked(file, corrupt).into()),
                }

                match bootstrap_token::read(paths.dirfd()) {
                    Ok(stored) if stored == expected_token => {}
                    Ok(_) => return Err(AdoptError::TokenMismatch),
                    Err(bootstrap_token::TokenReadError::NotFound) => {
                        return Err(AdoptError::AlreadyAdopted);
                    }
                    Err(bootstrap_token::TokenReadError::Corrupt(k)) => {
                        return Err(AdoptError::CorruptToken(k));
                    }
                    Err(bootstrap_token::TokenReadError::Io(e)) => {
                        return Err(ProcessError::Io(e).into());
                    }
                }

                publish_running_under_flock(file, &paths, &identity)?;

                // bootstrap_token cleanup: best-effort; refresh sweeps.
                if let Err(io) = bootstrap_token::delete(paths.dirfd()) {
                    tracing::warn!(
                        error = %io,
                        "bootstrap_token unlink failed after adoption; will be cleaned by refresh"
                    );
                }
                Ok(())
            })
        })
        .await;
        finalize_join(join)
    }
}

/// Shared post-precheck publish sequence used by both
/// [`ProcessStatusFile::locked_initial_write`] and
/// [`ProcessStatusFile::locked_adoption_write`].
///
/// 1. dirfd liveness (rev12 invariant 5).
/// 2. pid file publication via `pid_file::write_atomic_at`.
/// 3. In-place status rewrite to `Running`.
/// 4. `fsync` with one retry.
///
/// On any failure the rollback `Failed` write runs while the flock is still
/// held; the resulting [`SecondaryStatusWriteResult`] travels back inside
/// the relevant [`LockedSectionError`] variant. Returns
/// `LockedSectionError` so the call site can `.into()` into either
/// `StartupError` or `AdoptError` without duplicating routing logic.
fn publish_running_under_flock(
    file: &mut File,
    paths: &ProcPaths,
    identity: &ProcessIdentity,
) -> Result<(), LockedSectionError> {
    if proc_dir_vanished(paths.dirfd()) {
        return Err(LockedSectionError::ProcDirVanished {
            path: paths.dir().to_owned(),
        });
    }

    if let Err(e) = pid_file::write_atomic_at(paths.dirfd(), identity) {
        let secondary = best_effort_mark_failed(file);
        return Err(match e {
            PublishError::PidTmpResidue { source } => {
                LockedSectionError::PidTmpResidue { source, secondary }
            }
            PublishError::PidAlreadyPresent { source } => {
                LockedSectionError::PidAlreadyPresent { source, secondary }
            }
            PublishError::Io { source, step } => LockedSectionError::PidWriteFailed {
                source,
                step,
                path: paths.dir().join(names::PID),
                secondary,
            },
        });
    }

    if let Err(io) = write_status_in_place(file, ProcessStatus::Running) {
        let secondary = best_effort_mark_failed(file);
        return Err(LockedSectionError::StatusWriteFailed {
            source: io,
            secondary,
        });
    }

    if let Err(io) = fsync_with_one_retry(file) {
        let secondary = best_effort_mark_failed(file);
        return Err(LockedSectionError::StatusFsyncFailed {
            source: io,
            secondary,
        });
    }

    Ok(())
}

/// Map a [`CorruptStatusError`] observed inside a locked section to the
/// shared [`LockedSectionError::CorruptStatusOnRead`] shape, performing the
/// rollback `Failed` write before returning so the on-disk record is
/// left in a known-recoverable state.
fn corrupt_to_locked(file: &mut File, corrupt: CorruptStatusError) -> LockedSectionError {
    let CorruptStatusError { kind, raw_bytes } = corrupt;
    let secondary = best_effort_mark_failed(file);
    LockedSectionError::CorruptStatusOnRead {
        kind,
        raw_bytes,
        secondary,
    }
}

/// Project a `spawn_blocking` join result into the locked-section outer
/// error type, distinguishing panic (poison) from cancellation.
fn finalize_join<E>(join: Result<Result<(), E>, tokio::task::JoinError>) -> Result<(), E>
where
    E: From<LockedSectionError>,
{
    match join {
        Ok(inner) => inner,
        Err(je) if je.is_panic() => Err(LockedSectionError::JoinPanic.into()),
        Err(_) => Err(LockedSectionError::JoinCancelled.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::id::{Pid, ProcessId};
    use crate::process::paths::ProcPaths;
    use crate::process::pid_file::ProcessIdentity;
    use crate::process::proc_info::ProcessStartTime;
    use tempfile::TempDir;

    fn fake_identity() -> ProcessIdentity {
        if cfg!(target_os = "linux") {
            ProcessIdentity {
                pid: Pid::new(7777),
                start_time: ProcessStartTime::LinuxClockTicks(42),
                linux_boot_id: Some("0123456789abcdef-deadbeef".into()),
            }
        } else {
            ProcessIdentity {
                pid: Pid::new(7777),
                start_time: ProcessStartTime::MacosEpochMicros(1_700_000_000_000_000),
                linux_boot_id: None,
            }
        }
    }

    fn fresh_paths() -> (TempDir, Arc<ProcPaths>) {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let paths = ProcPaths::create_for_new_id(tmp.path(), id).unwrap();
        (tmp, paths)
    }

    #[tokio::test]
    async fn locked_initial_write_flips_to_running_and_publishes_pid() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let identity = fake_identity();
        sf.clone()
            .locked_initial_write(identity.clone(), paths.clone())
            .await
            .expect("startup");
        let observed = sf.read_status().await.expect("read");
        assert_eq!(observed, ProcessStatus::Running);
        let pid_body = std::fs::read_to_string(paths.join(names::PID)).unwrap();
        assert_eq!(pid_body, identity.to_pid_line());
    }

    #[tokio::test]
    async fn locked_initial_write_rejects_non_initializing() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        // Manually rewrite the status file to "killed" so the next attempt
        // hits the CancelledBeforeStart branch.
        std::fs::write(paths.join(names::STATUS), b"killed\n").unwrap();
        let err = sf
            .clone()
            .locked_initial_write(fake_identity(), paths.clone())
            .await
            .expect_err("must reject");
        assert!(matches!(err, StartupError::CancelledBeforeStart));
    }

    #[tokio::test]
    async fn locked_adoption_write_flips_running_and_deletes_token() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let token = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &token).expect("write token");
        sf.clone()
            .locked_adoption_write(fake_identity(), paths.clone(), token)
            .await
            .expect("adopt");
        let observed = sf.read_status().await.expect("read");
        assert_eq!(observed, ProcessStatus::Running);
        assert!(!bootstrap_token::exists(paths.dirfd()));
    }

    #[tokio::test]
    async fn locked_adoption_write_token_mismatch_rejects() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let real = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &real).expect("write");
        let other = BootstrapToken::generate();
        let err = sf
            .clone()
            .locked_adoption_write(fake_identity(), paths.clone(), other)
            .await
            .expect_err("must reject");
        assert!(matches!(err, AdoptError::TokenMismatch));
        // Status untouched — still Initializing.
        let observed = sf.read_status().await.expect("read");
        assert_eq!(observed, ProcessStatus::Initializing);
    }

    #[tokio::test]
    async fn locked_adoption_write_running_returns_already_adopted() {
        // Race: adopter A flips status to Running and (typically) clears
        // the token; adopter B observes Running and must report
        // AlreadyAdopted, not the internal UnexpectedStatus catch-all.
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let token = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &token).expect("write token");
        sf.clone()
            .locked_adoption_write(fake_identity(), paths.clone(), token)
            .await
            .expect("first adopt");
        let err = sf
            .clone()
            .locked_adoption_write(fake_identity(), paths.clone(), token)
            .await
            .expect_err("second adopt must reject");
        assert!(matches!(err, AdoptError::AlreadyAdopted));
    }

    #[tokio::test]
    async fn locked_adoption_write_no_token_returns_already_adopted() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        let token = BootstrapToken::generate();
        let err = sf
            .clone()
            .locked_adoption_write(fake_identity(), paths.clone(), token)
            .await
            .expect_err("must reject");
        assert!(matches!(err, AdoptError::AlreadyAdopted));
    }

    #[tokio::test]
    async fn locked_adoption_write_rejects_terminal_status() {
        let (_tmp, paths) = fresh_paths();
        let sf = ProcessStatusFile::create_initial_locked(paths.clone())
            .await
            .unwrap();
        std::fs::write(paths.join(names::STATUS), b"failed\n").unwrap();
        let token = BootstrapToken::generate();
        bootstrap_token::write_excl(paths.dirfd(), &token).unwrap();
        let err = sf
            .clone()
            .locked_adoption_write(fake_identity(), paths.clone(), token)
            .await
            .expect_err("must reject");
        assert!(matches!(err, AdoptError::ProcessAlreadyTerminated));
    }
}
