//! Error type for [`SandboxWorkspace`](super::SandboxWorkspace).

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by [`SandboxWorkspace`](super::SandboxWorkspace).
#[derive(Debug, Error)]
pub enum SandboxWorkspaceError {
    /// The configured base path does not exist on disk.
    #[error("sandbox workspace base path does not exist: {}", .0.display())]
    NotFound(PathBuf),
    /// The configured base path exists but is not a directory.
    #[error("sandbox workspace base path is not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// One of the configured glob patterns failed to compile.
    #[error("sandbox workspace glob pattern is invalid: {0}")]
    InvalidGlobPattern(#[from] globset::Error),
    /// Any other I/O error encountered while cloning or reconciling.
    #[error("sandbox workspace I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A project deny rule covers the Runner-owned temporary directory.
    #[error(
        "sandbox policy denies runner temporary directory {} via {}",
        path.display(),
        denied_by.display()
    )]
    RunnerTemporaryDirectoryDenied {
        /// Runner-owned temporary directory required by agent commands.
        path: PathBuf,
        /// Configured deny path that contains it.
        denied_by: PathBuf,
    },
    /// No sandbox backend is available for this platform.
    #[error("no sandbox backend for this platform")]
    UnsupportedPlatform,
    /// The operation was cancelled via the supplied
    /// [`CancellationToken`](tokio_util::sync::CancellationToken).
    #[error("sandbox workspace operation was cancelled")]
    Cancelled,
}
