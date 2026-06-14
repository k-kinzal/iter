//! `ObserverError` — failure surface of the lifecycle-observer layer.

use std::fmt;
use std::io;

/// Failure surface of the lifecycle observer (tracing-event emitter).
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum ObserverError {
    /// The dedicated lifecycle writer task has been dropped; further
    /// `observe()` calls cannot be persisted.
    WriterStopped,
    /// Generic I/O error from the observer write path.
    Io(io::Error),
}

impl fmt::Display for ObserverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObserverError::WriterStopped => {
                f.write_str("lifecycle observer writer task is no longer running")
            }
            ObserverError::Io(e) => write!(f, "observer I/O error: {e}"),
        }
    }
}

impl std::error::Error for ObserverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ObserverError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ObserverError {
    fn from(e: io::Error) -> Self {
        ObserverError::Io(e)
    }
}
