//! [`SandboxWorkspace`] — tmpdir clone wrapped by a kernel-level sandbox.
//!
//! The workspace is the strongest-isolation built-in option and the
//! layered counterpart to [`CloneWorkspace`](crate::workspace::CloneWorkspace):
//!
//! 1. **Clone.** The base directory is mirrored into a fresh
//!    [`tempfile::TempDir`], honoring the same
//!    [`ApplyBackMode`](crate::workspace::ApplyBackMode) / excludes / includes /
//!    `preserve_mtime` knobs [`CloneWorkspace`](crate::workspace::CloneWorkspace)
//!    exposes. The agent's *working* path is the tmpdir.
//! 2. **Wrap.** A platform-specific [`SandboxBackend`] generates an argv
//!    prefix (macOS `sandbox-exec`, Linux `bwrap`) that child processes
//!    must be spawned under. The prefix is retained on the workspace value
//!    and surfaced through
//!    [`Workspace::sandbox_command_prefix`](crate::Workspace::sandbox_command_prefix);
//!    the runner reads it after setup and threads it into the agent
//!    invocation as typed command-construction data — never through the
//!    process environment.
//!
//! # The two sides of the sandbox contract
//!
//! Every [`SandboxWorkspace`] is constructed with both a
//! [`SandboxPolicy`] (from the Iterfile) and a
//! [`SandboxRequirements`](crate::SandboxRequirements) (from the agent's
//! [`Agent::sandbox_requirements`](crate::Agent::sandbox_requirements)).
//! The policy is the project's **upper bound** — "this is what I'm
//! willing to let anything reach". The requirements are the agent's
//! **lower bound** — "this is what my process needs to work at all".
//! The backend merges the two and the workspace fails closed at
//! construction if the agent's floor exceeds the project's ceiling.
//!
//! The clone layer keeps modification-time and copy-back semantics
//! identical to [`CloneWorkspace`](crate::workspace::CloneWorkspace), so
//! a clone-only workspace and a sandbox-wrapped clone workspace can be
//! compared without a workspace-shape confound.
//!
//! # No project-shaped defaults
//!
//! The constructor takes every knob explicitly. There is no `Default`
//! impl on [`SandboxPolicy`]; "network off or network on" is a
//! project-shaped decision and iter refuses to pick for the project.
//!
//! # Platform support
//!
//! | Host            | Backend                                                     |
//! | --------------- | ----------------------------------------------------------- |
//! | macOS           | [`SandboxExecBackend`](backend::macos::SandboxExecBackend)  |
//! | Linux           | [`BwrapBackend`](backend::linux::BwrapBackend)              |
//! | everything else | [`SandboxWorkspaceError::UnsupportedPlatform`]              |
//!
//! On platforms without a built-in backend, [`Workspace::setup`](crate::Workspace::setup)
//! fails fast. Callers that want graceful skipping (e.g. CI) should check
//! [`SandboxWorkspace::detect_backend_available`] up front.

pub mod agent_requirements;
pub mod backend;
pub mod error;
pub mod policy;
pub mod requirements;
pub mod workspace;

pub use backend::{BackendError, SandboxBackend, SandboxDescriptor};
pub use error::SandboxWorkspaceError;
pub use policy::{NetworkAccess, SandboxPolicy};
pub use requirements::{SandboxRequirements, match_env_pattern};
pub use workspace::SandboxWorkspace;
