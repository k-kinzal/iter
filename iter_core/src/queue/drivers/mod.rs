//! Queue driver implementations.
//!
//! Each subdirectory is a self-contained driver that implements
//! [`crate::queue::Queue`]. Cloud-backend drivers are gated behind feature
//! flags so `cargo check -p iter_core --no-default-features` compiles
//! without pulling in heavy SDK dependencies.

// Shared AWS utilities (credentials, HTTP client) used by the SQS and
// Kinesis drivers. Only compiled when at least one AWS driver is enabled.
#[cfg(any(feature = "driver-sqs", feature = "driver-kinesis"))]
pub mod aws;

#[cfg(feature = "driver-sqs")]
pub mod sqs;

#[cfg(feature = "driver-kinesis")]
pub mod kinesis;

#[cfg(feature = "driver-pubsub")]
pub mod pubsub;

#[cfg(feature = "driver-servicebus")]
pub mod servicebus;

#[cfg(feature = "driver-kafka")]
pub mod kafka;

#[cfg(feature = "driver-redis")]
pub mod redis;

pub mod file;
pub mod memory;
pub mod shell;
