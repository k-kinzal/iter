//! `workspace` declaration AST and supporting sandbox-policy types.

/// Reconciliation strategy for `workspace clone` teardown.
///
/// Mirrors the `ApplyBackMode` enum in `iter_workspace` but lives in the
/// DSL layer so the language crate stays independent of the workspace
/// implementation crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneApplyBackMode {
    /// Copy temp→base, delete base files that disappeared in temp.
    Sync,
    /// Never reconcile. Temp is dropped on teardown.
    Discard,
    /// Copy new/modified files temp→base but never delete.
    Merge,
}

/// Apply-back configuration: reconciliation [`mode`](Self::mode) plus the
/// teardown-time filter pair (`excludes` / `includes`).
///
/// The filter is independent from the clone-time filter on the parent
/// workspace block — a path can be visible to the agent in the temp tree
/// but blocked from propagating back on teardown (the motivating
/// asymmetric-filtering use case). `excludes` and `includes` must be
/// empty when [`mode`](Self::mode) is `Discard`; this is enforced by the
/// semantic analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyBackDecl {
    /// Reconciliation strategy.
    pub mode: CloneApplyBackMode,
    /// Apply-back-time exclude pattern list. Empty = no exclusions.
    pub excludes: Vec<String>,
    /// Apply-back-time include override list. Entries here win over
    /// matching entries in `excludes`. Empty = no overrides.
    pub includes: Vec<String>,
}

/// Workspace backend declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDecl {
    /// Run agents directly inside the existing directory at `base`.
    Local {
        /// Filesystem path to the workspace root. Required.
        base: String,
    },
    /// Clone the directory at `base` (or `remote` if set) into a fresh
    /// scratch directory before each run.
    Clone {
        /// Source directory or path used as the clone seed. Required.
        base: String,
        /// Optional remote URL passed to whatever clone backend the project
        /// configures. iter does not interpret the string.
        remote: Option<String>,
        /// Clone-time exclude pattern list. Required. `[]` explicitly
        /// disables skipping.
        excludes: Vec<String>,
        /// Clone-time include override list. Entries bypass the exclude
        /// list. Empty means "no overrides".
        includes: Vec<String>,
        /// Whether the clone preserves the source files' modification
        /// times. Required.
        preserve_mtime: bool,
        /// Reconciliation block (mode + apply-back-time filter).
        /// Required.
        apply_back: ApplyBackDecl,
    },
    /// Run agents inside a sandboxed copy of `base`. The sandbox is a
    /// tmpdir clone (honouring the same excludes / includes / mtime /
    /// apply-back knobs as [`Clone`](WorkspaceDecl::Clone)) wrapped by a
    /// kernel-level sandbox (`sandbox-exec` on macOS, `bwrap` on Linux).
    Sandbox {
        /// Source directory used to seed the sandbox. Required.
        base: String,
        /// Clone-time exclude pattern list. Required.
        excludes: Vec<String>,
        /// Clone-time include override list. Entries bypass the exclude
        /// list.
        includes: Vec<String>,
        /// Whether to preserve modification times during the clone phase.
        /// Required.
        preserve_mtime: bool,
        /// Reconciliation block (mode + apply-back-time filter).
        /// Required.
        apply_back: ApplyBackDecl,
        /// Workspace-level sandbox policy (upper-bound rules). Required.
        policy: SandboxPolicyDecl,
    },
}

/// Network-access rule applied by the sandbox.
///
/// There is no default: network access is a project-shaped decision (some
/// projects cannot function without it, others must run isolated). The
/// source file must spell it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxNetworkDecl {
    /// No outbound network.
    Off,
    /// Unrestricted outbound network.
    All,
    /// Network allowed only to the listed hostnames (unioned with the
    /// agent's declared `network_hosts`).
    Hosts(Vec<String>),
}

/// Workspace-level sandbox policy (upper bound).
///
/// The four `Vec<String>` fields are additive over the agent's declared
/// `sandbox_requirements` (the lower bound). Empty vectors mean "no project
/// additions" — the common case and what you get when you omit the field.
/// `network` is required: there is no honest default (`off` breaks some
/// projects, `all` breaks others).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicyDecl {
    /// Network-access rule. Required.
    pub network: SandboxNetworkDecl,
    /// Absolute paths outside the workspace tmpdir the agent may read.
    pub allow_read_outside: Vec<String>,
    /// Absolute paths outside the workspace tmpdir the agent may write.
    pub allow_write_outside: Vec<String>,
    /// Absolute paths explicitly denied.
    pub extra_deny_paths: Vec<String>,
    /// Absolute paths to binaries the sandbox may execve. Empty means
    /// "inherit backend default".
    pub allow_exec: Vec<String>,
}
