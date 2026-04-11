//! `AnyWorkspace` enum + the `build_workspace_factory` builder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iter_core::workspace::{
    ApplyBackMode, CloneSettings, CloneWorkspace, CloneWorkspaceError, LocalWorkspace,
    LocalWorkspaceError, SandboxPolicy, SandboxWorkspace, SandboxWorkspaceError,
};
use iter_core::{SandboxRequirements, Workspace};
use iter_language::{ApplyBackDecl, CloneApplyBackMode, WorkspaceDecl};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Enum dispatch wrapper over every concrete [`iter_core::Workspace`]
/// implementation.
#[derive(Debug)]
pub enum AnyWorkspace {
    /// Direct-on-disk [`LocalWorkspace`].
    Local(LocalWorkspace),
    /// Temp-directory [`CloneWorkspace`].
    Clone(Box<CloneWorkspace>),
    /// Container-backed [`SandboxWorkspace`].
    Sandbox(Box<SandboxWorkspace>),
}

/// Aggregated error type returned by [`AnyWorkspace`]'s [`Workspace`] impl.
#[derive(Debug, Error)]
pub enum AnyWorkspaceError {
    /// Forwarded error from [`LocalWorkspace`].
    #[error(transparent)]
    Local(LocalWorkspaceError),
    /// Forwarded error from [`CloneWorkspace`].
    #[error(transparent)]
    Clone(CloneWorkspaceError),
    /// Forwarded error from [`SandboxWorkspace`].
    #[error(transparent)]
    Sandbox(SandboxWorkspaceError),
}

impl Workspace for AnyWorkspace {
    type Error = AnyWorkspaceError;

    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), Self::Error> {
        match self {
            Self::Local(w) => w.setup(cancel).await.map_err(AnyWorkspaceError::Local),
            Self::Clone(w) => w.setup(cancel).await.map_err(AnyWorkspaceError::Clone),
            Self::Sandbox(w) => w.setup(cancel).await.map_err(AnyWorkspaceError::Sandbox),
        }
    }

    async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), Self::Error> {
        match self {
            Self::Local(w) => w.teardown(cancel).await.map_err(AnyWorkspaceError::Local),
            Self::Clone(w) => w.teardown(cancel).await.map_err(AnyWorkspaceError::Clone),
            Self::Sandbox(w) => w.teardown(cancel).await.map_err(AnyWorkspaceError::Sandbox),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Local(w) => w.path(),
            Self::Clone(w) => w.path(),
            Self::Sandbox(w) => w.path(),
        }
    }

    fn final_path(&self) -> &Path {
        match self {
            Self::Local(w) => w.final_path(),
            Self::Clone(w) => w.final_path(),
            Self::Sandbox(w) => w.final_path(),
        }
    }
}

/// Frozen workspace configuration extracted from an [`Iterfile`](iter_language::Root).
///
/// We snapshot the AST values into a `Send + Sync` payload so the
/// workspace factory closure can be cloned across threads
/// without holding a reference to the AST itself.
///
/// Every project-shaped knob is stored unconditionally here — there are
/// no `Option`s. The AST already enforces explicit values (iter ships no
/// project-shaped defaults), so the compose layer is a straight 1:1 copy.
#[derive(Debug, Clone)]
struct SandboxSpec {
    base: PathBuf,
    excludes: Vec<String>,
    includes: Vec<String>,
    preserve_mtime: bool,
    apply_back: ApplyBackMode,
    apply_back_excludes: Vec<String>,
    apply_back_includes: Vec<String>,
    policy: SandboxPolicy,
    requirements: SandboxRequirements,
}

#[derive(Debug, Clone)]
enum WorkspaceSpec {
    Local {
        base: PathBuf,
    },
    Clone {
        base: PathBuf,
        excludes: Vec<String>,
        includes: Vec<String>,
        preserve_mtime: bool,
        apply_back: ApplyBackMode,
        apply_back_excludes: Vec<String>,
        apply_back_includes: Vec<String>,
    },
    Sandbox(Box<SandboxSpec>),
}

impl WorkspaceSpec {
    fn instantiate(&self) -> AnyWorkspace {
        match self {
            Self::Local { base } => AnyWorkspace::Local(LocalWorkspace::new(base.clone())),
            Self::Clone {
                base,
                excludes,
                includes,
                preserve_mtime,
                apply_back,
                apply_back_excludes,
                apply_back_includes,
            } => AnyWorkspace::Clone(Box::new(CloneWorkspace::new(
                base.clone(),
                CloneSettings {
                    excludes: excludes.clone(),
                    includes: includes.clone(),
                    preserve_mtime: *preserve_mtime,
                    apply_back: *apply_back,
                    apply_back_excludes: apply_back_excludes.clone(),
                    apply_back_includes: apply_back_includes.clone(),
                },
            ))),
            Self::Sandbox(spec) => AnyWorkspace::Sandbox(Box::new(SandboxWorkspace::new(
                spec.base.clone(),
                CloneSettings {
                    excludes: spec.excludes.clone(),
                    includes: spec.includes.clone(),
                    preserve_mtime: spec.preserve_mtime,
                    apply_back: spec.apply_back,
                    apply_back_excludes: spec.apply_back_excludes.clone(),
                    apply_back_includes: spec.apply_back_includes.clone(),
                },
                spec.policy.clone(),
                spec.requirements.clone(),
            ))),
        }
    }
}

fn map_apply_back_mode(mode: CloneApplyBackMode) -> ApplyBackMode {
    match mode {
        CloneApplyBackMode::Sync => ApplyBackMode::Sync,
        CloneApplyBackMode::Discard => ApplyBackMode::Discard,
        CloneApplyBackMode::Merge => ApplyBackMode::Merge,
    }
}

fn map_sandbox_policy(decl: &iter_language::SandboxPolicyDecl) -> SandboxPolicy {
    use iter_core::workspace::NetworkAccess;
    use iter_language::SandboxNetworkDecl;
    SandboxPolicy {
        network: match &decl.network {
            SandboxNetworkDecl::Off => NetworkAccess::Off,
            SandboxNetworkDecl::All => NetworkAccess::All,
            SandboxNetworkDecl::Hosts(hosts) => NetworkAccess::Hosts(hosts.clone()),
        },
        allow_read_outside: decl.allow_read_outside.iter().map(PathBuf::from).collect(),
        allow_write_outside: decl.allow_write_outside.iter().map(PathBuf::from).collect(),
        extra_deny_paths: decl.extra_deny_paths.iter().map(PathBuf::from).collect(),
        allow_exec: decl.allow_exec.iter().map(PathBuf::from).collect(),
    }
}

/// Build a workspace factory from a [`WorkspaceDecl`].
///
/// The returned closure clones the captured spec on every call so that each
/// signal sees a fresh, not-yet-set-up workspace — exactly the contract
/// demanded by [`iter_core::Runner`].
///
/// `agent_requirements` is the agent's declared lower bound (what the agent
/// needs to function) and is merged into every
/// [`SandboxWorkspace`](iter_core::workspace::SandboxWorkspace) the factory
/// yields. The workspace policy (the project's upper bound) comes from the
/// DSL; the requirements come from the agent. For non-sandbox workspaces the
/// parameter is unused.
///
/// The factory closure is constructed eagerly here; setup-time validation
/// is deferred to [`Workspace::setup`] on the produced workspace, which is
/// why this function is infallible.
#[must_use = "the returned factory closure is not useful unless called"]
pub fn build_workspace_factory(
    decl: &WorkspaceDecl,
    agent_requirements: SandboxRequirements,
) -> impl Fn() -> AnyWorkspace + Send + Sync + use<> {
    let spec = match decl {
        WorkspaceDecl::Local { base } => WorkspaceSpec::Local {
            base: PathBuf::from(base),
        },
        WorkspaceDecl::Clone {
            base,
            remote: _,
            excludes,
            includes,
            preserve_mtime,
            apply_back:
                ApplyBackDecl {
                    mode,
                    excludes: ab_excludes,
                    includes: ab_includes,
                },
        } => WorkspaceSpec::Clone {
            base: PathBuf::from(base),
            excludes: excludes.clone(),
            includes: includes.clone(),
            preserve_mtime: *preserve_mtime,
            apply_back: map_apply_back_mode(*mode),
            apply_back_excludes: ab_excludes.clone(),
            apply_back_includes: ab_includes.clone(),
        },
        WorkspaceDecl::Sandbox {
            base,
            excludes,
            includes,
            preserve_mtime,
            apply_back:
                ApplyBackDecl {
                    mode,
                    excludes: ab_excludes,
                    includes: ab_includes,
                },
            policy,
        } => WorkspaceSpec::Sandbox(Box::new(SandboxSpec {
            base: PathBuf::from(base),
            excludes: excludes.clone(),
            includes: includes.clone(),
            preserve_mtime: *preserve_mtime,
            apply_back: map_apply_back_mode(*mode),
            apply_back_excludes: ab_excludes.clone(),
            apply_back_includes: ab_includes.clone(),
            policy: map_sandbox_policy(policy),
            requirements: agent_requirements,
        })),
    };

    let spec = Arc::new(spec);
    move || spec.instantiate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_language::{SandboxNetworkDecl, SandboxPolicyDecl, WorkspaceDecl};

    #[test]
    fn factory_yields_distinct_local_instances() {
        let decl = WorkspaceDecl::Local {
            base: "/tmp/iter-cli-test".into(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let a = factory();
        let b = factory();
        match (a, b) {
            (AnyWorkspace::Local(_), AnyWorkspace::Local(_)) => {}
            other => panic!("expected two LocalWorkspaces, got {other:?}"),
        }
    }

    fn sync_apply_back() -> ApplyBackDecl {
        ApplyBackDecl {
            mode: CloneApplyBackMode::Sync,
            excludes: Vec::new(),
            includes: Vec::new(),
        }
    }

    #[test]
    fn factory_handles_clone_decl() {
        let decl = WorkspaceDecl::Clone {
            base: "/tmp/iter-cli-test".into(),
            remote: None,
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert!(matches!(w, AnyWorkspace::Clone(_)));
    }

    #[test]
    fn factory_handles_clone_with_remote() {
        let decl = WorkspaceDecl::Clone {
            base: "/tmp/iter-cli-test".into(),
            remote: Some("https://example.com/repo".into()),
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert!(matches!(w, AnyWorkspace::Clone(_)));
    }

    #[test]
    fn factory_handles_sandbox_decl() {
        let decl = WorkspaceDecl::Sandbox {
            base: "/tmp/iter-cli-test".into(),
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
            policy: SandboxPolicyDecl {
                network: SandboxNetworkDecl::Off,
                allow_read_outside: Vec::new(),
                allow_write_outside: Vec::new(),
                extra_deny_paths: Vec::new(),
                allow_exec: Vec::new(),
            },
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert!(matches!(w, AnyWorkspace::Sandbox(_)));
    }
}
