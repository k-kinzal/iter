//! Workspace implementations for the iter agent control framework.
//!
//! Three implementations directly realize the workspace breadth model:
//!
//! - [`LocalWorkspace`] — the target directory itself (widest exploration).
//! - [`CloneWorkspace`] — a filesystem copy; changes apply back on teardown
//!   according to an [`ApplyBackMode`].
//! - [`SandboxWorkspace`] — a filesystem copy wrapped by a kernel-level
//!   sandbox (macOS `sandbox-exec` / Linux `bwrap`); strongest isolation.
//!
//! All three implement [`crate::Workspace`].
//!
//! # Which one should I use?
//!
//! | Implementation       | Isolation     | Persistence | Notes                                   |
//! | -------------------- | ------------- | ----------- | --------------------------------------- |
//! | [`LocalWorkspace`]   | none          | direct      | widest context; riskiest                |
//! | [`CloneWorkspace`]   | fs only       | apply-back  | safe default for exploratory work       |
//! | [`SandboxWorkspace`] | fs + kernel   | apply-back  | needs sandbox-exec/bwrap; tightest      |
//!
//! See the individual module docs for details.

pub mod apply_back;
pub mod clone;
pub mod error;
pub mod inner;
pub mod local;
pub(crate) mod mirror;
pub mod sandbox;

pub use error::WorkspaceError;
pub use inner::Workspace;

pub use apply_back::ApplyBackMode;
pub use clone::{CloneSettings, CloneWorkspace, CloneWorkspaceError};
pub use local::{LocalWorkspace, LocalWorkspaceError};
pub use sandbox::{
    BackendError, ITER_SANDBOX_COMMAND_PREFIX, NetworkAccess, SANDBOX_PREFIX_SEP, SandboxBackend,
    SandboxDescriptor, SandboxPolicy, SandboxRequirements, SandboxWorkspace, SandboxWorkspaceError,
    current_sandbox_prefix, decode_prefix_env, encode_prefix_env, match_env_pattern,
};
