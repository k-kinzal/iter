//! Error type for [`CloneWorkspace`](super::CloneWorkspace).

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by [`CloneWorkspace`](super::CloneWorkspace).
#[derive(Debug, Error)]
pub enum CloneWorkspaceError {
    /// The configured base path does not exist on disk.
    #[error("clone workspace base path does not exist: {}", .0.display())]
    NotFound(PathBuf),
    /// The configured base path exists but is not a directory.
    #[error("clone workspace base path is not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// The workspace has not been set up.
    #[error("clone workspace has not been set up")]
    NotSetUp,
    /// One of the configured glob patterns failed to compile.
    #[error("clone workspace glob pattern is invalid: {0}")]
    InvalidGlobPattern(#[from] globset::Error),
    /// Any other I/O error encountered while copying files.
    #[error("clone workspace I/O error: {0}")]
    Io(#[from] std::io::Error),
}
