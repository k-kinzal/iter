//! Re-exports from the runner-owned lifecycle and agent-owned outcome
//! modules.
//!
//! `RunnerLifecycle`, `RedactedMetadata`, and `AgentOutcomeKind` are
//! defined by the layers that own them (runner and agent respectively).
//! This module re-exports them so existing `crate::process::lifecycle::*`
//! paths continue to compile.

pub use crate::agent::AgentOutcomeKind;
pub use crate::runner::lifecycle::{RedactedMetadata, RunnerLifecycle};
