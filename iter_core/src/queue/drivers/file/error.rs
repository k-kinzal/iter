//! Errors produced by [`FileQueue`](super::FileQueue).

use thiserror::Error;

/// Errors produced by [`FileQueue`](super::FileQueue).
#[derive(Debug, Error)]
pub enum FileQueueError {
    /// Filesystem error while creating directories or moving signal files.
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to (de)serialize a signal payload.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    /// The underlying `notify` watcher failed to start or attach to the
    /// pending directory.
    #[error("watcher error: {0}")]
    Watcher(#[from] notify::Error),
    /// `queue` was called after the queue had been `close`d.
    #[error("cannot enqueue to a closed queue")]
    Closed,
}
