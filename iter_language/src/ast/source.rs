//! `source` declaration AST.

/// Exploration-scoped provenance and disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDef {
    /// Filesystem directory source.
    Directory {
        /// Canonical directory path.
        path: String,
        /// How to derive the runner base from the canonical directory.
        derive: SourceDerive,
        /// How to dispose the runner base when the exploration finishes.
        disposition: Option<SourceDisposition>,
    },
    /// Git-backed source.
    Git {
        /// Local path or remote URL locator.
        locator: GitLocator,
        /// How to derive the runner base from the git source.
        derive: SourceDerive,
        /// How to dispose the runner base when the exploration finishes.
        disposition: SourceDisposition,
    },
}

/// Git source locator. Exactly one is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitLocator {
    /// Remote git URL.
    Url(String),
    /// Local git repository path.
    Path(String),
}

/// Provisioning strategy from canonical source to durable exploration base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDerive {
    /// Use the canonical path directly.
    Passthrough,
    /// Copy a directory once at runner start.
    Copy {
        /// Copy-time exclude patterns.
        excludes: Vec<String>,
        /// Preserve source mtimes in the copy.
        preserve_mtime: bool,
    },
    /// Add a git worktree.
    Worktree {
        /// Ref to derive from. Defaults to `HEAD`.
        ref_name: Option<String>,
        /// Branch name for the worktree. When absent, runtime generates one.
        branch: Option<String>,
    },
    /// Clone a git repository.
    Clone {
        /// Ref to check out. Defaults to the clone's default branch.
        ref_name: Option<String>,
        /// Branch name for the clone. When absent, runtime generates one.
        branch: Option<String>,
        /// Optional shallow clone depth.
        depth: Option<u64>,
    },
}

/// Exploration-finish disposition from durable base back to canonical source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDisposition {
    /// Drop the durable base; leave canonical untouched.
    Discard,
    /// Non-destructive fold into canonical.
    Merge {
        /// Exclude patterns for directory merge/sync.
        excludes: Vec<String>,
        /// Include whitelist for directory merge/sync.
        includes: Vec<String>,
        /// Target branch/path-specific destination. For git, defaults to the source ref branch.
        into: Option<String>,
        /// Git fast-forward policy.
        ff: Option<GitFastForward>,
    },
    /// Destructive sync into canonical.
    Sync {
        /// Exclude patterns for directory sync.
        excludes: Vec<String>,
        /// Include whitelist for directory sync.
        includes: Vec<String>,
    },
    /// Park the base and record a later promote/discard decision.
    Defer {
        /// Disposition to run when the operator promotes the parked base.
        promote: Box<SourceDisposition>,
    },
}

/// Git merge fast-forward policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFastForward {
    /// Allow fast-forward or merge commit.
    Allow,
    /// Require fast-forward.
    Only,
    /// Always create a merge commit.
    No,
}

/// Workspace-side reference to a source declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSourceRef {
    /// Named top-level source block.
    Named(String),
    /// Inline path sugar: `source = "/path"`.
    Path(String),
}
