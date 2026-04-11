//! Error type for the command trigger.

use iter_core::MetadataError;
use thiserror::Error;

/// Errors produced by [`CommandTrigger`](super::CommandTrigger).
#[derive(Debug, Error)]
pub enum CommandTriggerError<E: std::error::Error + Send + Sync + 'static> {
    /// Forwarded error from the queue backing the trigger.
    #[error("queue error: {0}")]
    Queue(#[source] E),

    /// I/O error spawning or talking to the subprocess.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parsing failed for a record or for the configured extractor.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The configured regex failed to compile.
    #[error("invalid regex: {0}")]
    Regex(String),

    /// The configured shell could not be parsed (e.g. empty string).
    #[error("invalid shell: {0}")]
    InvalidShell(String),

    /// The polled command exited non-zero and `on_error = abort` is in effect.
    #[error("command exited non-zero (on_error = abort): {0}")]
    Aborted(String),

    /// Construction of an internal metadata key failed, or a record produced
    /// a key that violates [`MetadataKey`](iter_core::MetadataKey) constraints.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),
}
