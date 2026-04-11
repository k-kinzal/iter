//! Error type for [`LocalWorkspace`](super::LocalWorkspace).

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by [`LocalWorkspace`](super::LocalWorkspace).
#[derive(Debug, Error)]
pub enum LocalWorkspaceError {
    /// The configured base path does not exist on disk.
    #[error("local workspace path does not exist: {}", .0.display())]
    NotFound(PathBuf),
    /// The configured base path exists but is not a directory.
    #[error("local workspace path is not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// Any other I/O error encountered during validation.
    #[error("local workspace I/O error: {0}")]
    Io(#[from] std::io::Error),
}
