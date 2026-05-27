//! Re-exports from the runner-owned lifecycle and agent-owned result
//! modules.
//!
//! `RunnerLifecycle`, `RedactedMetadata`, and `AgentResultKind` are
//! defined by the layers that own them (runner and agent respectively).
//! This module re-exports them so existing `crate::process::lifecycle::*`
//! paths continue to compile.

pub use crate::agent::AgentResultKind;
pub use crate::runner::lifecycle::{RedactedMetadata, RunnerLifecycle};
