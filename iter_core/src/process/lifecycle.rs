//! Re-exports from the runner-owned lifecycle and agent-owned result
//! modules.
//!
//! `RunnerLifecycle` and `RedactedMetadata` are defined by the runner layer
//! that owns them.
//! This module re-exports them so existing `crate::process::lifecycle::*`
//! paths continue to compile.

pub use crate::runner::lifecycle::{RedactedMetadata, RunnerLifecycle};
