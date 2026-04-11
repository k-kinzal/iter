//! `ProcessError` — the outer type returned by every public `async fn` in
//! the `process` subsystem.
//!
//! `Startup(Box<_>)` / `Adopt(Box<_>)` carry the typed inner errors raised
//! inside the flock-protected critical section. The indirection breaks the
//! otherwise-recursive sized-type relationship with
//! [`super::locked_section::LockedSectionError::Io`].

use std::fmt;
use std::io;

use crate::process::id::ProcessId;
use crate::process::status::{CorruptStatusError, ProcessStatus};

use super::adopt::AdoptError;
use super::startup::StartupError;

/// Outer error surface for the `process` subsystem.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProcessError {
    /// `flock(LOCK_EX)` failed on the status file.
    FlockAcquire(io::Error),
    /// `flock(LOCK_UN)` failed on the status file. Surfaced when the primary
    /// operation succeeded; if the primary itself failed, this is downgraded
    /// to a `tracing::warn!` so the typed primary error reaches the caller
    /// unmasked.
    FlockRelease(io::Error),
    /// The intra-process `Mutex<File>` was poisoned by a panic inside a
    /// previous critical section. Treated as unrecoverable: every subsequent
    /// helper call on the same `ProcessStatusFile` returns this variant.
    StatusFilePoisoned,
    /// `read_status` decoded a corrupt body. Carried at the outer layer for
    /// the rare paths that observe corruption *outside* `locked_initial_write`
    /// / `locked_adoption_write` (where
    /// [`super::locked_section::LockedSectionError::CorruptStatusOnRead`] is
    /// used instead).
    CorruptStatus(CorruptStatusError),
    /// `transition(from, to)` was rejected — either the on-disk status did
    /// not match the caller's `from`, or `is_allowed(from, to)` is false
    /// (e.g. anyone trying to reach `Running` via the generic transition
    /// path; that path is reserved for `locked_initial_write` /
    /// `locked_adoption_write`).
    IllegalTransition {
        /// What the caller expected to find on disk.
        from: ProcessStatus,
        /// Where the caller wanted to move.
        to: ProcessStatus,
        /// What we actually read on disk; `None` when the read matched
        /// `from` but the `from→to` edge itself was disallowed.
        observed: Option<ProcessStatus>,
    },
    /// A precondition check rejected an operation because the on-disk
    /// status was not terminal (`Stopped` / `Failed` / `Killed`). Distinct
    /// from [`Self::IllegalTransition`] in that *no* transition was even
    /// attempted — the caller looked at the current state and refused to
    /// proceed (e.g. `Handle::remove` will not unlink a still-running
    /// proc directory).
    NotTerminal {
        /// Status observed at the time of the precondition check.
        current: ProcessStatus,
    },
    /// pid-file body failed to parse (wrong OS prefix, malformed numbers,
    /// `boot_id` mismatch length, …). Lifecycle evidence: the caller treats it
    /// as Failed after the grace period.
    CorruptPidFile {
        /// Verbatim file contents (truncated to a bounded length).
        raw_bytes: Vec<u8>,
        /// One-line reason suitable for diagnostics.
        reason: String,
    },
    /// Linux: `/proc/sys/kernel/random/boot_id` could not be read. Treated
    /// as fatal at registration / adoption time so we never silently accept
    /// a Linux record without a boot id.
    UnsupportedProcIdentity {
        /// Why we cannot identify the running process.
        reason: String,
    },
    /// Generic I/O failure observed by the subsystem (e.g. while opening the
    /// proc directory, reading a side file, etc.). The wrapped `io::Error`
    /// carries the errno.
    Io(io::Error),
    /// Failed to serialise `meta.json` (or another structured side-file).
    JsonWrite(serde_json::Error),
    /// Failed to deserialise `meta.json` from disk.
    JsonRead(serde_json::Error),
    /// `meta.json::id` disagrees with the directory the metadata was loaded
    /// from. Indicates external tampering or a partial-write recovery race.
    MetadataIdMismatch {
        /// The id the directory is named after.
        expected: ProcessId,
        /// The id the on-disk `meta.json` claims.
        found: ProcessId,
    },
    /// Forwarded `StartupError` from `locked_initial_write`.
    Startup(Box<StartupError>),
    /// Forwarded `AdoptError` from `locked_adoption_write`.
    Adopt(Box<AdoptError>),
    /// The current platform (Windows) does not support the subsystem.
    UnsupportedPlatform,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::FlockAcquire(e) => write!(f, "flock(LOCK_EX) failed: {e}"),
            ProcessError::FlockRelease(e) => write!(f, "flock(LOCK_UN) failed: {e}"),
            ProcessError::StatusFilePoisoned => {
                f.write_str("status file mutex poisoned by a previous panic")
            }
            ProcessError::CorruptStatus(e) => write!(f, "{e}"),
            ProcessError::IllegalTransition { from, to, observed } => match observed {
                Some(o) => write!(
                    f,
                    "illegal status transition: expected from={from}, observed {o}; cannot move to {to}"
                ),
                None => write!(
                    f,
                    "illegal status transition: {from} -> {to} is not allowed"
                ),
            },
            ProcessError::NotTerminal { current } => write!(
                f,
                "operation requires terminal status (Stopped/Failed/Killed); observed {current}"
            ),
            ProcessError::CorruptPidFile { reason, .. } => {
                write!(f, "corrupt pid file: {reason}")
            }
            ProcessError::UnsupportedProcIdentity { reason } => {
                write!(f, "unsupported process identity: {reason}")
            }
            ProcessError::Io(e) => write!(f, "I/O error: {e}"),
            ProcessError::JsonWrite(e) => write!(f, "failed to serialise JSON: {e}"),
            ProcessError::JsonRead(e) => write!(f, "failed to deserialise JSON: {e}"),
            ProcessError::MetadataIdMismatch { expected, found } => write!(
                f,
                "meta.json id mismatch: directory={expected}, file claims={found}"
            ),
            ProcessError::Startup(inner) => write!(f, "startup failed: {inner}"),
            ProcessError::Adopt(inner) => write!(f, "adoption failed: {inner}"),
            ProcessError::UnsupportedPlatform => f.write_str("platform not supported"),
        }
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProcessError::FlockAcquire(e) | ProcessError::FlockRelease(e) | ProcessError::Io(e) => {
                Some(e)
            }
            ProcessError::CorruptStatus(e) => Some(e),
            ProcessError::Startup(inner) => Some(&**inner),
            ProcessError::Adopt(inner) => Some(&**inner),
            ProcessError::JsonWrite(e) | ProcessError::JsonRead(e) => Some(e),
            ProcessError::StatusFilePoisoned
            | ProcessError::IllegalTransition { .. }
            | ProcessError::NotTerminal { .. }
            | ProcessError::CorruptPidFile { .. }
            | ProcessError::UnsupportedProcIdentity { .. }
            | ProcessError::MetadataIdMismatch { .. }
            | ProcessError::UnsupportedPlatform => None,
        }
    }
}

impl From<io::Error> for ProcessError {
    fn from(e: io::Error) -> Self {
        ProcessError::Io(e)
    }
}

impl From<CorruptStatusError> for ProcessError {
    fn from(e: CorruptStatusError) -> Self {
        ProcessError::CorruptStatus(e)
    }
}
