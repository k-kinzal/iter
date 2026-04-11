//! `RegistryError` — failure surface of the `name_lock` / `registry`
//! layer. Owned by the registry rather than mixed into `ProcessError`
//! because its consumers (`Handle::register`, the CLI) react to specific
//! variants (`AlreadyExists`, `UnsupportedFilesystem`) rather than to a
//! generic I/O blob.

use std::fmt;
use std::io;

/// Failure surface of the `name_lock` / `registry` layer.
#[derive(Debug)]
#[non_exhaustive]
pub enum RegistryError {
    /// Name failed validation (forbidden characters, leading dot, length, …).
    InvalidName {
        /// One-line reason.
        reason: String,
    },
    /// Another live process is already registered under this name.
    AlreadyExists,
    /// `linkat(2)` returned `EPERM` / `EOPNOTSUPP` / `EXDEV` — the
    /// underlying filesystem does not support the atomic-link primitive
    /// the registry depends on (NFS, some cross-device mounts).
    UnsupportedFilesystem,
    /// `linkat(2)` returned `ENOENT` 3 times in a row, indicating the tmp
    /// file disappears before the link can be made. Almost always a sign of
    /// a runaway third-party cleanup process.
    TmpRetryExhausted,
    /// Lock body could not be parsed and the grace period has not yet
    /// elapsed.
    CorruptLock,
    /// Generic I/O error escaping from the registry path.
    Io(io::Error),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::InvalidName { reason } => write!(f, "invalid name: {reason}"),
            RegistryError::AlreadyExists => f.write_str("name already in use"),
            RegistryError::UnsupportedFilesystem => {
                f.write_str("filesystem does not support `linkat(2)` (NFS / cross-device?)")
            }
            RegistryError::TmpRetryExhausted => {
                f.write_str("`linkat` source vanished 3 times in a row; aborting")
            }
            RegistryError::CorruptLock => f.write_str("lock body could not be parsed"),
            RegistryError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegistryError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for RegistryError {
    fn from(e: io::Error) -> Self {
        RegistryError::Io(e)
    }
}
