//! Queue trait and driver implementations.
//!
//! The [`Queue`] trait lives at this level alongside shared cross-backend
//! types ([`envelope`], [`retry`], [`dlq`], [`Priority`]). Concrete
//! implementations live under [`drivers`], gated behind feature flags for
//! cloud backends.
//!
//! Always-available drivers (no external SDK dependency):
//!
//! - [`drivers::memory::InMemoryQueue`] — single-process default backed by a
//!   `BinaryHeap` and [`tokio::sync::Notify`].
//! - [`drivers::file::FileQueue`] — persistent directory-of-files queue using
//!   POSIX atomic `rename(2)`.
//! - [`drivers::shell::ShellQueue`] — escape hatch where users supply
//!   `enqueue` / `dequeue` shell commands in the Iterfile.
//!
//! Feature-gated cloud drivers:
//!
//! - `driver-sqs` — Amazon SQS.
//! - `driver-kinesis` — Amazon Kinesis Data Streams.
//! - `driver-pubsub` — GCP Cloud Pub/Sub.
//! - `driver-servicebus` — Azure Service Bus.
//! - `driver-kafka` — Apache Kafka.
//! - `driver-redis` — Redis sorted-set queue.

pub mod inner;

pub use inner::Queue;

pub mod drivers;

// ─── Re-exports for ergonomic access ────────────────────────────────────

pub use drivers::file::{FileQueue, FileQueueError};
pub use drivers::memory::{InMemoryQueue, InMemoryQueueError};
pub use drivers::shell::{ShellQueue, ShellQueueConfig, ShellQueueError};

#[cfg(feature = "driver-redis")]
pub use drivers::redis::{RedisQueue, RedisQueueError};

#[cfg(any(feature = "driver-sqs", feature = "driver-kinesis"))]
pub use drivers::aws;

#[cfg(feature = "driver-sqs")]
pub use drivers::sqs;

#[cfg(feature = "driver-kinesis")]
pub use drivers::kinesis;

#[cfg(feature = "driver-pubsub")]
pub use drivers::pubsub as gcp;

#[cfg(feature = "driver-servicebus")]
pub use drivers::servicebus as azure;

#[cfg(feature = "driver-kafka")]
pub use drivers::kafka;

// ─── Shared cross-backend types ─────────────────────────────────────────

pub mod priority;

pub use priority::Priority;

pub mod envelope;

pub use envelope::{Envelope, EnvelopeError, decode_signal, encode_signal};

pub mod retry;

pub use retry::{RetryMode, RetryPolicy};

pub mod dlq;

pub use dlq::{DlqKind, DlqPolicy, DlqTarget};
