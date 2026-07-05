//! [`SandboxWorkspace`] — tmpdir clone whose spawns are kernel-confined.
//!
//! The workspace is the strongest-isolation built-in option and the
//! layered counterpart to [`CloneWorkspace`](crate::workspace::CloneWorkspace):
//!
//! 1. **Clone.** The base directory is mirrored into a fresh
//!    [`tempfile::TempDir`], honoring the same
//!    [`ApplyBackMode`](crate::workspace::ApplyBackMode) / excludes / includes /
//!    `preserve_mtime` knobs [`CloneWorkspace`](crate::workspace::CloneWorkspace)
//!    exposes. The agent's *working* path is the tmpdir.
//! 2. **Confine.** Setup builds a structured [`sandbox::Policy`]; the active
//!    workspace's [`spawn`](crate::workspace::ActiveWorkspace::spawn) wraps
//!    every child command with it through the `sandbox` crate (macOS
//!    `sandbox-exec`, Linux `bwrap`). Isolation is a property of process
//!    creation, so it is applied exactly there — the agent side never sees
//!    the word "sandbox".
//!
//! # The two sides of the sandbox contract
//!
//! Every [`SandboxWorkspace`] is constructed with both a [`SandboxPolicy`]
//! (from the declaration) and a [`SandboxProfile`] (assembled from the
//! agent's drivers at start time). A driver reports only object-safe *facts*
//! — its [`kind`](crate::agent::AgentDriver::kind),
//! [`executable_read_paths`](crate::agent::AgentDriver::executable_read_paths), and
//! [`declared_env`](crate::agent::AgentDriver::declared_env) — and
//! [`SandboxProfile::for_drivers`] matches **exhaustively** over the closed
//! [`AgentKind`](crate::agent::AgentKind) to build the profile, so adding an
//! agent kind without a sandbox arm is a compile error.
//!
//! The policy is the project's **upper bound** — "this is what I'm willing to
//! let anything reach". The profile is the agent's **lower bound** — "this is
//! what my process needs to work at all". Setup merges the two into one
//! [`sandbox::Policy`].
//!
//! The clone layer keeps modification-time and copy-back semantics
//! identical to [`CloneWorkspace`](crate::workspace::CloneWorkspace), so
//! a clone-only workspace and a sandbox-confined clone workspace can be
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
//! | Host            | Confinement host command      |
//! | --------------- | ----------------------------- |
//! | macOS           | `sandbox-exec` (via `sandbox`) |
//! | Linux           | `bwrap` (via `sandbox`)        |
//! | everything else | [`SandboxWorkspaceError::UnsupportedPlatform`] |
//!
//! On platforms without a confinement host,
//! [`Workspace::setup`](crate::Workspace::setup) fails fast. Callers that
//! want graceful skipping (e.g. CI) should check
//! [`SandboxWorkspace::detect_backend_available`] up front.

pub mod error;
pub mod policy;
pub mod profile;
pub mod workspace;

pub use error::SandboxWorkspaceError;
pub use policy::{NetworkAccess, SandboxPolicy};
pub use profile::{SandboxProfile, match_env_pattern};
pub use workspace::SandboxWorkspace;
