//! Error type for [`SandboxWorkspace`](super::SandboxWorkspace).

use std::path::PathBuf;

use thiserror::Error;

use super::backend::BackendError;

/// Errors produced by [`SandboxWorkspace`](super::SandboxWorkspace).
#[derive(Debug, Error)]
pub enum SandboxWorkspaceError {
    /// The configured base path does not exist on disk.
    #[error("sandbox workspace base path does not exist: {}", .0.display())]
    NotFound(PathBuf),
    /// The configured base path exists but is not a directory.
    #[error("sandbox workspace base path is not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// The workspace has not been set up.
    #[error("sandbox workspace has not been set up")]
    NotSetUp,
    /// The sandbox backend failed to prepare or cleanup.
    #[error("sandbox backend error: {0}")]
    Backend(#[from] BackendError),
    /// One of the configured glob patterns failed to compile.
    #[error("sandbox workspace glob pattern is invalid: {0}")]
    InvalidGlobPattern(#[from] globset::Error),
    /// Any other I/O error encountered while cloning or reconciling.
    #[error("sandbox workspace I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// No sandbox backend is available for this platform.
    #[error("no sandbox backend for this platform")]
    UnsupportedPlatform,
    /// The operation was cancelled via the supplied
    /// [`CancellationToken`](tokio_util::sync::CancellationToken).
    #[error("sandbox workspace operation was cancelled")]
    Cancelled,
}
