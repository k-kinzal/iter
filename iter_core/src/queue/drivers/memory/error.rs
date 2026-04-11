//! Errors produced by [`InMemoryQueue`](super::InMemoryQueue).

use thiserror::Error;

/// Errors produced by [`InMemoryQueue`](super::InMemoryQueue).
#[derive(Debug, Error)]
pub enum InMemoryQueueError {
    /// `queue` was called after the queue had been `close`d. Producers
    /// that see this should stop enqueuing; consumers will drain the
    /// remaining signals and then see `Ok(None)` from `dequeue`.
    #[error("cannot enqueue to a closed queue")]
    Closed,
}
