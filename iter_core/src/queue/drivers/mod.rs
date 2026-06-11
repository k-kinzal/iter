//! Queue driver implementations.
//!
//! Each subdirectory is a self-contained driver that implements
//! [`crate::queue::Queue`]. Cloud-backend drivers are gated behind feature
//! flags so `cargo check -p iter_core --no-default-features` compiles
//! without pulling in heavy SDK dependencies.

// Shared AWS utilities (credentials, HTTP client) used by the SQS driver.
#[cfg(feature = "driver-sqs")]
pub mod aws;

#[cfg(feature = "driver-sqs")]
pub mod sqs;

#[cfg(feature = "driver-redis")]
pub mod redis;

pub mod file;
pub mod memory;
pub mod shell;
