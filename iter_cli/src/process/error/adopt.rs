//! `AdoptError` — failure surface of `locked_adoption_write` (detached
//! child adoption atomic critical section).
//!
//! Four entry-check variants are unique to adoption; the post-handshake
//! publication failures are delegated to the shared
//! [`super::locked_section::LockedSectionError`] so foreground and
//! detached cannot drift apart.

use std::fmt;

use super::locked_section::LockedSectionError;
use super::process::ProcessError;
use super::token::TokenCorruptKind;

/// Failure surface of `locked_adoption_write` (detached child adoption).
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum AdoptError {
    /// Status was already terminal (`Stopped` / `Failed` / `Killed`) when the
    /// flock was acquired. Mapped to a 0-exit by the CLI.
    ProcessAlreadyTerminated,
    /// `adoption_token` file is missing; this id has already been adopted.
    AlreadyAdopted,
    /// `adoption_token` was present but did not match the value the parent
    /// passed via the well-known file.
    TokenMismatch,
    /// `adoption_token` failed validation (length / hex / case).
    CorruptToken(TokenCorruptKind),
    /// A failure raised from the shared locked critical section.
    LockedSection(LockedSectionError),
}

impl fmt::Display for AdoptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdoptError::ProcessAlreadyTerminated => f.write_str("process already terminated"),
            AdoptError::AlreadyAdopted => f.write_str("already adopted"),
            AdoptError::TokenMismatch => f.write_str("adoption token mismatch"),
            AdoptError::CorruptToken(kind) => write!(f, "corrupt adoption token: {kind:?}"),
            AdoptError::LockedSection(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AdoptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdoptError::LockedSection(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LockedSectionError> for AdoptError {
    fn from(e: LockedSectionError) -> Self {
        AdoptError::LockedSection(e)
    }
}

impl From<ProcessError> for AdoptError {
    fn from(e: ProcessError) -> Self {
        AdoptError::LockedSection(LockedSectionError::Io(e))
    }
}
