//! `LockedSectionError` — failures shared by `locked_initial_write`
//! (foreground startup) and `locked_adoption_write` (detached adoption).
//!
//! Both sections walk the same on-disk publication sequence — read status,
//! write `.pid.tmp`, `linkat` it to `pid`, write `Running`, `fsync` —
//! so each step's failure mode is the same regardless of how the section
//! was entered. Sharing the surface here removes the symmetry-by-copy
//! hazard the rev17 review flagged.
//!
//! Variants ending in `secondary` carry the result of the rollback
//! `Failed` write so callers can distinguish "Failed reached disk" from
//! "rollback itself failed".

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::process::pid_file::PublishStep;
use crate::process::status::{CorruptStatusKind, ProcessStatus};

use super::process::ProcessError;
use super::secondary::SecondaryStatusWriteResult;

/// Shared failure surface of the locked critical section walked by both
/// `locked_initial_write` and `locked_adoption_write`. See module-level
/// docs for the inner / outer layering.
#[derive(Debug)]
#[non_exhaustive]
pub enum LockedSectionError {
    /// Status read a token that is decodable but not allowed at this
    /// point (e.g. a startup running into an existing `Running` record).
    UnexpectedStatus(ProcessStatus),
    /// `read_status` returned [`crate::process::status::CorruptStatusError`].
    CorruptStatusOnRead {
        /// Why the body was rejected.
        kind: CorruptStatusKind,
        /// Verbatim file contents at observation.
        raw_bytes: Vec<u8>,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// `openat(.pid.tmp, O_CREAT|O_EXCL)` returned `EEXIST`.
    PidTmpResidue {
        /// Underlying I/O error (errno).
        source: io::Error,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// `linkat(.pid.tmp → pid)` returned `EEXIST`.
    PidAlreadyPresent {
        /// Underlying I/O error (errno).
        source: io::Error,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// pid-file write failed at one of the [`PublishStep`] sites.
    PidWriteFailed {
        /// Underlying I/O error (errno).
        source: io::Error,
        /// The step at which the failure occurred.
        step: PublishStep,
        /// Path of the pid file (for diagnostics).
        path: PathBuf,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// `write_status_in_place(Running)` failed at the `write_all` step.
    StatusWriteFailed {
        /// Underlying I/O error.
        source: io::Error,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// `fsync(status_file)` failed (one retry already attempted).
    StatusFsyncFailed {
        /// Underlying I/O error.
        source: io::Error,
        /// Result of the rollback `Failed` write.
        secondary: SecondaryStatusWriteResult,
    },
    /// `dirfd` of the proc directory is no longer valid (`fstat` reports
    /// `nlink == 0`, or returns `EBADF` / `ENODEV` / `ESTALE`). The
    /// rollback `Failed` write is *not* attempted because the file lives
    /// under that same directory.
    ProcDirVanished {
        /// Path of the (now-missing) directory, for diagnostics.
        path: PathBuf,
    },
    /// `tokio::task::JoinError::is_panic()`. Implies the intra-process
    /// `Mutex` is poisoned.
    JoinPanic,
    /// `tokio::task::JoinError` was cancelled (runtime shutdown counts
    /// here because Tokio's API conflates the two).
    JoinCancelled,
    /// Pre-flock environmental I/O failure (flock acquire, mutex poison,
    /// open, seek). Carries the outer [`ProcessError`] payload.
    Io(ProcessError),
}

impl fmt::Display for LockedSectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockedSectionError::UnexpectedStatus(s) => write!(f, "unexpected status on read: {s}"),
            LockedSectionError::CorruptStatusOnRead { kind, .. } => {
                write!(f, "corrupt status on read: {kind:?}")
            }
            LockedSectionError::PidTmpResidue { source, .. } => {
                write!(f, "stale `.pid.tmp` present: {source}")
            }
            LockedSectionError::PidAlreadyPresent { source, .. } => {
                write!(f, "`pid` already present: {source}")
            }
            LockedSectionError::PidWriteFailed {
                source, step, path, ..
            } => write!(
                f,
                "pid write failed at {step:?} ({}): {source}",
                path.display()
            ),
            LockedSectionError::StatusWriteFailed { source, .. } => {
                write!(f, "status write failed: {source}")
            }
            LockedSectionError::StatusFsyncFailed { source, .. } => {
                write!(f, "status fsync failed: {source}")
            }
            LockedSectionError::ProcDirVanished { path } => {
                write!(f, "proc dir vanished: {}", path.display())
            }
            LockedSectionError::JoinPanic => f.write_str("locked task panicked"),
            LockedSectionError::JoinCancelled => f.write_str("locked task cancelled"),
            LockedSectionError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LockedSectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LockedSectionError::PidTmpResidue { source, .. }
            | LockedSectionError::PidAlreadyPresent { source, .. }
            | LockedSectionError::PidWriteFailed { source, .. }
            | LockedSectionError::StatusWriteFailed { source, .. }
            | LockedSectionError::StatusFsyncFailed { source, .. } => Some(source),
            LockedSectionError::Io(e) => Some(e),
            LockedSectionError::CorruptStatusOnRead { kind: _, .. }
            | LockedSectionError::UnexpectedStatus(_)
            | LockedSectionError::ProcDirVanished { .. }
            | LockedSectionError::JoinPanic
            | LockedSectionError::JoinCancelled => None,
        }
    }
}

impl From<ProcessError> for LockedSectionError {
    fn from(e: ProcessError) -> Self {
        LockedSectionError::Io(e)
    }
}
