//! Errors returned by the Kafka backend.

use thiserror::Error;

/// Errors returned by the Kafka backend.
#[derive(Debug, Error)]
pub enum KafkaQueueError {
    /// The configuration was internally inconsistent.
    #[error("config error: {0}")]
    Config(String),
    /// The Kafka runtime path is not yet wired in the current build.
    #[error(
        "Kafka `{operation}` is not yet implemented; the DSL surface is stable but the rdkafka runtime wiring lands in a follow-up release"
    )]
    NotYetImplemented {
        /// Name of the operation the caller invoked.
        operation: &'static str,
    },
    /// `queue()` was called after `close()`.
    #[error("queue is closed")]
    Closed,
}
