//! `StartupError` — failure surface of `locked_initial_write` (foreground
//! startup atomic critical section).
//!
//! Two entry-check variants are unique to startup; everything else is
//! delegated to the shared [`super::locked_section::LockedSectionError`].

use std::fmt;

use super::locked_section::LockedSectionError;
use super::process::ProcessError;

/// Failure surface of `locked_initial_write` (foreground startup).
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum StartupError {
    /// Status was already `Killed` when the flock was acquired (the user
    /// pre-empted the start). Mapped to a 0-exit by the CLI.
    CancelledBeforeStart,
    /// Status was already `Failed` when the flock was acquired. Treated as
    /// idempotent: the runtime exits with 1.
    AlreadyMarkedFailed,
    /// A failure raised from the shared locked critical section.
    LockedSection(LockedSectionError),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartupError::CancelledBeforeStart => f.write_str("cancelled before start"),
            StartupError::AlreadyMarkedFailed => f.write_str("status already Failed"),
            StartupError::LockedSection(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StartupError::LockedSection(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LockedSectionError> for StartupError {
    fn from(e: LockedSectionError) -> Self {
        StartupError::LockedSection(e)
    }
}

impl From<ProcessError> for StartupError {
    fn from(e: ProcessError) -> Self {
        StartupError::LockedSection(LockedSectionError::Io(e))
    }
}
