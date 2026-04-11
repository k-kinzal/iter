//! Errors produced by [`RedisQueue`](super::RedisQueue).

use thiserror::Error;

/// Errors produced by [`RedisQueue`](super::RedisQueue).
#[derive(Debug, Error)]
pub enum RedisQueueError {
    /// Underlying Redis driver error.
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    /// Failed to (de)serialize a signal payload.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
