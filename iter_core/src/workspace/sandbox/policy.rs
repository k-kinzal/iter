//! [`SandboxPolicy`] — workspace-level sandbox configuration.
//!
//! # Two sides of the sandbox contract
//!
//! Inside a [`SandboxWorkspace`](super::SandboxWorkspace) two independent
//! declarations meet:
//!
//! 1. **[`SandboxPolicy`] — the project's upper bound.** The policy comes
//!    from the declaration (via the `workspace sandbox { policy { ... } }`
//!    block) and describes *what the project is willing to let the agent
//!    reach*. A policy with [`NetworkAccess::Off`] disables network
//!    regardless of what the agent asks for.
//!
//! 2. **[`SandboxRequirements`](crate::SandboxRequirements) — the agent's
//!    lower bound.** Each supported agent declares, via
//!    [`Agent::sandbox_requirements`](crate::Agent::sandbox_requirements),
//!    the set of paths, hosts, and env vars its process needs to function
//!    at all. iter ships this knowledge so declaration authors never have to
//!    enumerate it themselves.
//!
//! The workspace merges both at setup. The merge is *intersection* on the
//! network axis and *union* on the allow-list axes, but always clipped by
//! the project's deny rules — so the policy is always the ceiling and the
//! agent requirements are always the floor. If the agent's floor exceeds
//! the project's ceiling the sandbox fails closed at construction time;
//! silent downgrades would be a worse result than refusing to run.
//!
//! # No project-shaped defaults
//!
//! [`SandboxPolicy`] has no `Default` impl. Every field is mandatory at
//! the constructor because "what should my project let the agent reach?"
//! is a decision iter cannot honestly make for the project. Network
//! access in particular has no default — some workflows require it,
//! others require the opposite, and silently defaulting to either side
//! is a footgun.

use std::path::PathBuf;

/// Network-access policy applied by the sandbox.
///
/// This type has no `Default` impl. Every sandbox-backed workspace must
/// declare its posture explicitly because iter cannot guess whether a
/// given project can function without network access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkAccess {
    /// No outbound network. Overrides any
    /// [`network_hosts`](crate::SandboxRequirements::network_hosts)
    /// the agent declares.
    Off,
    /// Network allowed only to the hosts listed here and to the subset
    /// of the agent's declared
    /// [`network_hosts`](crate::SandboxRequirements::network_hosts)
    /// that also appear here. If the list is empty, only the intersection
    /// with the agent's declarations is allowed — effectively
    /// "whatever the agent declared, nothing else".
    Hosts(Vec<String>),
    /// Unrestricted outbound network. The loosest setting; use with care.
    All,
}

/// Workspace-level sandbox policy. Authors edit this via the
/// `workspace sandbox { policy { ... } }` DSL block.
///
/// No `Default` impl: construct by populating every field explicitly.
/// The project is the only party that can honestly pick values here.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Network-access rule, applied before the agent's declared hosts.
    pub network: NetworkAccess,

    /// Absolute paths outside the workspace tmpdir the agent may **read**.
    ///
    /// Entries are passed to the backend verbatim; subdirectories are
    /// included recursively. The agent's declared
    /// [`file_reads`](crate::SandboxRequirements::file_reads) are
    /// union'd in on top.
    pub allow_read_outside: Vec<PathBuf>,

    /// Absolute paths outside the workspace tmpdir the agent may **write**.
    ///
    /// Keep this tight — every entry is a hole in the sandbox. The
    /// agent's declared
    /// [`file_writes`](crate::SandboxRequirements::file_writes) are
    /// union'd in on top.
    pub allow_write_outside: Vec<PathBuf>,

    /// Absolute paths explicitly denied even if something else (policy
    /// or agent declaration) would otherwise permit them.
    ///
    /// Deny takes precedence over allow: this list is the right knob for
    /// "I don't care what the agent asks for, stay away from X".
    pub extra_deny_paths: Vec<PathBuf>,

    /// Absolute paths to binaries the sandbox may `execve`.
    ///
    /// Empty means "inherit the backend's default", which typically
    /// allows everything on `$PATH`. Populating this list switches the
    /// sandbox to an allow-list for binaries.
    pub allow_exec: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_off_policy_has_empty_allow_lists() {
        let p = SandboxPolicy {
            network: NetworkAccess::Off,
            allow_read_outside: Vec::new(),
            allow_write_outside: Vec::new(),
            extra_deny_paths: Vec::new(),
            allow_exec: Vec::new(),
        };
        assert!(matches!(p.network, NetworkAccess::Off));
        assert!(p.allow_read_outside.is_empty());
        assert!(p.allow_write_outside.is_empty());
        assert!(p.extra_deny_paths.is_empty());
        assert!(p.allow_exec.is_empty());
    }
}
