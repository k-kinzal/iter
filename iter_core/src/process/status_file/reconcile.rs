//! Refresh-time reconciliation of `<dir>/status` against on-disk evidence.
//!
//! The body of `ProcessStatusFile::reconcile_under_lock` lives here: given a
//! locked `&mut File`, the directory's `dirfd`, the `started_at` recorded in
//! `meta.json`, and the bootstrap grace, decide whether the process record
//! should remain in its current state, be transitioned to `Failed`, or be
//! left untouched while transient I/O issues clear.
//!
//! All sub-paths walk the rev17 §C3 reconciliation tree. The function is
//! pure-sync and does not perform any locking — the caller (the parent
//! `mod.rs`) is responsible for the `flock` + `Mutex<File>` discipline.

use std::fs::File;
use std::os::fd::BorrowedFd;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::process::bootstrap_token;
use crate::process::error::ProcessError;
use crate::process::pid_file::{self, PidFileState};
use crate::process::proc_info;
use crate::process::status::ProcessStatus;

use super::body::{fsync_with_one_retry, read_status, write_status_in_place};

/// Body of `reconcile_under_lock`, executed under both the intra-process
/// `Mutex<File>` and the cross-process `flock(LOCK_EX)`. Single critical
/// section per rev17 §C3.
pub(super) fn reconcile_inner(
    file: &mut File,
    dirfd: BorrowedFd<'_>,
    started_at: DateTime<Utc>,
    grace: Duration,
) -> Result<ProcessStatus, ProcessError> {
    let now = Utc::now();
    let grace_elapsed = grace_elapsed_saturating(started_at, now, grace);

    match read_status(file) {
        // ── Top-level corrupt status (rev13 §B1) ────────────────────────
        Err(corrupt) => {
            tracing::warn!(
                kind = ?corrupt.kind,
                "corrupt status observed during refresh"
            );
            if grace_elapsed {
                mark_failed_with_full_cleanup(file, dirfd)?;
                Ok(ProcessStatus::Failed)
            } else {
                Err(ProcessError::CorruptStatus(corrupt))
            }
        }

        // ── Initializing ────────────────────────────────────────────────
        //
        // All three lifecycle outcomes (`NotFound`, `Corrupt`, `Found`)
        // converge on the same decision: stay `Initializing` while in
        // grace, flip to `Failed` once grace elapses. We deliberately do
        // not call `process_is_alive_*` even on `Found` — its result does
        // not change the outcome, and probing a possibly-dead pid is
        // wasted syscall traffic. Only environmental errors fall through
        // to a tracing warning with no on-disk change.
        Ok(ProcessStatus::Initializing) => match pid_file::read(dirfd) {
            PidFileState::NotFound | PidFileState::Corrupt(_) | PidFileState::Found(_) => {
                if grace_elapsed {
                    mark_failed_with_full_cleanup(file, dirfd)?;
                    Ok(ProcessStatus::Failed)
                } else {
                    Ok(ProcessStatus::Initializing)
                }
            }
            other => {
                tracing::warn!(
                    state = ?other,
                    "pid_file env error during refresh (status=initializing)"
                );
                Ok(ProcessStatus::Initializing)
            }
        },

        // ── Running ─────────────────────────────────────────────────────
        Ok(ProcessStatus::Running) => match pid_file::read(dirfd) {
            // C1 invariant: pid is published *before* Running.
            // Absence ⇒ lifecycle failure. Corrupt content ⇒ same fate.
            PidFileState::NotFound | PidFileState::Corrupt(_) => {
                mark_failed_with_full_cleanup(file, dirfd)?;
                Ok(ProcessStatus::Failed)
            }
            PidFileState::Found(rec) => match proc_info::process_is_alive_with_start_time(&rec) {
                Ok(true) => {
                    drop(bootstrap_token::delete(dirfd));
                    if pid_file::pid_residue_predicate(dirfd) {
                        drop(pid_file::delete_pid_tmp(dirfd));
                    }
                    Ok(ProcessStatus::Running)
                }
                Ok(false) => {
                    mark_failed_with_full_cleanup(file, dirfd)?;
                    Ok(ProcessStatus::Failed)
                }
                Err(e) => {
                    // Aliveness probe failed transiently — bias toward
                    // "still running" rather than write a false Failed.
                    tracing::warn!(error = %e, "process aliveness probe failed; staying Running");
                    Ok(ProcessStatus::Running)
                }
            },
            other => {
                tracing::warn!(
                    state = ?other,
                    "pid_file env error during refresh (status=running)"
                );
                Ok(ProcessStatus::Running)
            }
        },

        // ── Terminal ────────────────────────────────────────────────────
        Ok(terminal) => {
            cleanup_terminal_residue(dirfd);
            Ok(terminal)
        }
    }
}

/// Single-critical-section transition to `Failed` plus the full residue
/// cleanup the rev17 §C3 tree mandates: `bootstrap_token`, `.pid.tmp`, and
/// the `linkat` partial-adoption residue (`pid_residue_predicate` true).
fn mark_failed_with_full_cleanup(
    file: &mut File,
    dirfd: BorrowedFd<'_>,
) -> Result<(), ProcessError> {
    write_status_in_place(file, ProcessStatus::Failed).map_err(ProcessError::Io)?;
    fsync_with_one_retry(file).map_err(ProcessError::Io)?;
    cleanup_terminal_residue(dirfd);
    Ok(())
}

/// Best-effort cleanup applied to terminal records (Stopped / Failed /
/// Killed) and to fresh transitions into Failed. Both calls treat `ENOENT`
/// as success.
///
/// Unlike the Running branch in [`reconcile_inner`] — which deletes
/// `.pid.tmp` only when [`pid_file::pid_residue_predicate`] confirms the
/// `linkat` partial-adoption pattern — terminal cleanup runs under flock,
/// so no concurrent publish can be in flight. `.pid.tmp` is therefore
/// removed unconditionally; any residue, whether `linkat`-paired with the
/// recorded pid or left over from an earlier interrupted publish, is
/// reaped in the same critical section.
fn cleanup_terminal_residue(dirfd: BorrowedFd<'_>) {
    drop(bootstrap_token::delete(dirfd));
    drop(pid_file::delete_pid_tmp(dirfd));
}

/// Saturating-compare grace check that biases toward "still in grace" under
/// any clock anomaly.
///
/// Two protections:
/// 1. **Backward jump** (`elapsed < 0`): the system clock moved before
///    `started_at`. Treat as "not elapsed" so we don't false-Fail a still-
///    booting record (rev5 §C3 doc'd skew bias).
/// 2. **Forward jump** (`elapsed > SKEW_CAP_SECS`): a year+ of elapsed time
///    almost certainly means a wall-clock jump (`started_at` was wrong, or
///    the clock just moved). Treat as "not elapsed" rather than reaping
///    every record on the next tick.
fn grace_elapsed_saturating(
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
    grace: Duration,
) -> bool {
    const SKEW_CAP_SECS: i64 = 365 * 24 * 60 * 60;
    let elapsed = now.signed_duration_since(started_at).num_seconds();
    if !(0..=SKEW_CAP_SECS).contains(&elapsed) {
        return false;
    }
    u64::try_from(elapsed).expect("elapsed bounded above by SKEW_CAP_SECS") >= grace.as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grace_not_elapsed_when_within_window() {
        let started = Utc::now();
        let now = started + chrono::Duration::seconds(5);
        assert!(!grace_elapsed_saturating(
            started,
            now,
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn grace_elapsed_when_past_window() {
        let started = Utc::now() - chrono::Duration::seconds(60);
        let now = Utc::now();
        assert!(grace_elapsed_saturating(
            started,
            now,
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn grace_not_elapsed_when_clock_jumps_backward() {
        // started_at is *after* now → negative elapsed.
        let now = Utc::now();
        let started = now + chrono::Duration::seconds(10);
        assert!(!grace_elapsed_saturating(
            started,
            now,
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn grace_not_elapsed_when_clock_jumps_forward_more_than_a_year() {
        let started = Utc::now() - chrono::Duration::seconds(365 * 24 * 60 * 60 + 1);
        let now = Utc::now();
        // > SKEW_CAP_SECS → bias to "still in grace" instead of false-Failing
        // every existing record on the next refresh.
        assert!(!grace_elapsed_saturating(
            started,
            now,
            Duration::from_secs(30)
        ));
    }
}
