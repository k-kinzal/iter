//! [`CloneWorkspace`] — filesystem copy of a base directory.
//!
//! Instead of letting the agent operate directly on the base directory, a
//! [`CloneWorkspace`] mirrors the directory into a fresh
//! [`tempfile::TempDir`] and hands that to the agent. Depending on the
//! configured [`ApplyBackMode`](crate::workspace::ApplyBackMode), changes
//! made by the agent may be copied back on teardown, discarded outright,
//! or merged conservatively.
//!
//! This sits between [`LocalWorkspace`](crate::workspace::LocalWorkspace) and
//! [`SandboxWorkspace`](crate::workspace::SandboxWorkspace) on the
//! "isolation vs. fidelity" spectrum:
//!
//! - No process isolation (the agent still runs on the host).
//! - Filesystem isolation via a temp directory.
//! - Configurable reconciliation of results back into the base.
//!
//! # No project-shaped defaults
//!
//! Every knob is mandatory at construction time: [`CloneSettings`] has no
//! `Default` impl and no implicit values. Which directory basenames count
//! as "junk" (build output, dependency caches, per-tool scratch) is a
//! project-shaped decision — iter does not guess on the project's behalf.
//! Callers supply `excludes = vec![]` to mean "copy everything" or an
//! explicit list of basenames to skip.

pub mod error;
pub mod settings;
pub mod workspace;

pub use error::CloneWorkspaceError;
pub use settings::CloneSettings;
pub use workspace::CloneWorkspace;
