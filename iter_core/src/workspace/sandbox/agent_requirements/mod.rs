//! Per-agent [`SandboxRequirements`](crate::SandboxRequirements) builders.
//!
//! This module owns the "Workspace-side" half of the sandbox contract:
//! given a concrete agent type, it assembles the minimum OS-level access
//! profile the agent needs to function inside a
//! [`SandboxWorkspace`](super::SandboxWorkspace). The agent types
//! themselves expose only individual path accessors (config files,
//! scratch dirs, binary location); the allow-list policy (network hosts,
//! env passthrough patterns, signal escalation) lives here because it is
//! an environment-shaped concern, not an agent-shaped one.
//!
//! See the module-level docs on
//! [`SandboxPolicy`](super::SandboxPolicy) for the "upper bound vs. lower
//! bound" framing: this module produces the lower-bound declaration that
//! the workspace merges against the project's upper-bound policy.

pub mod claude;
pub mod grok;

pub use claude::claude;
pub use grok::grok;
