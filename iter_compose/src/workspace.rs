//! The per-iteration workspace supply: `build_workspace_factory` translates a
//! [`WorkspaceDef`] into a closure that mints a fresh `Box<dyn Workspace>` for
//! every signal.
//!
//! The runtime workspace axis is a trait object (R18): the closed set of
//! workspace kinds lives at the definition layer ([`WorkspaceDef`]); at run
//! time the runner only needs "something that sets up", so the supply yields
//! `Box<dyn Workspace>`. There is no run-time enum wrapper.

use std::path::PathBuf;
use std::sync::Arc;

use iter_core::workspace::{
    ApplyBackMode, CloneSettings, CloneWorkspace, LocalWorkspace, SandboxPolicy, SandboxWorkspace,
};
use iter_core::{SandboxRequirements, Workspace};
use iter_language::{ApplyBackDef, CloneApplyBackMode, WorkspaceDef};

/// Frozen workspace configuration extracted from an [`Iterfile`](iter_language::Iterfile).
///
/// We snapshot the AST values into a `Send + Sync` payload so the
/// workspace supply closure can be cloned across threads
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
    fn instantiate(&self) -> Box<dyn Workspace> {
        match self {
            Self::Local { base } => Box::new(LocalWorkspace::new(base.clone())),
            Self::Clone {
                base,
                excludes,
                includes,
                preserve_mtime,
                apply_back,
                apply_back_excludes,
                apply_back_includes,
            } => Box::new(CloneWorkspace::new(
                base.clone(),
                CloneSettings {
                    excludes: excludes.clone(),
                    includes: includes.clone(),
                    preserve_mtime: *preserve_mtime,
                    apply_back: *apply_back,
                    apply_back_excludes: apply_back_excludes.clone(),
                    apply_back_includes: apply_back_includes.clone(),
                },
            )),
            Self::Sandbox(spec) => Box::new(SandboxWorkspace::new(
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
            )),
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

fn map_sandbox_policy(decl: &iter_language::SandboxPolicyDef) -> SandboxPolicy {
    use iter_core::workspace::NetworkAccess;
    use iter_language::SandboxNetworkDef;
    SandboxPolicy {
        network: match &decl.network {
            SandboxNetworkDef::Off => NetworkAccess::Off,
            SandboxNetworkDef::All => NetworkAccess::All,
            SandboxNetworkDef::Hosts(hosts) => NetworkAccess::Hosts(hosts.clone()),
        },
        allow_read_outside: decl.allow_read_outside.iter().map(PathBuf::from).collect(),
        allow_write_outside: decl.allow_write_outside.iter().map(PathBuf::from).collect(),
        extra_deny_paths: decl.extra_deny_paths.iter().map(PathBuf::from).collect(),
        allow_exec: decl.allow_exec.iter().map(PathBuf::from).collect(),
    }
}

/// Build the per-iteration workspace supply from a [`WorkspaceDef`].
///
/// The returned closure clones the captured spec on every call so that each
/// signal sees a fresh, not-yet-set-up `Box<dyn Workspace>` — exactly the
/// contract demanded by [`iter_core::Runner`].
///
/// `agent_requirements` is the agent's declared lower bound (what the agent
/// needs to function) and is merged into every
/// [`SandboxWorkspace`](iter_core::workspace::SandboxWorkspace) the supply
/// yields. The workspace policy (the project's upper bound) comes from the
/// DSL; the requirements come from the agent. For non-sandbox workspaces the
/// parameter is unused.
///
/// The closure is constructed eagerly here; setup-time validation is deferred
/// to [`Workspace::setup`] on the produced workspace, which is why this
/// function is infallible.
#[must_use = "the returned workspace supply closure is not useful unless called"]
pub fn build_workspace_factory(
    decl: &WorkspaceDef,
    agent_requirements: SandboxRequirements,
) -> impl Fn() -> Box<dyn Workspace> + Send + Sync + use<> {
    let spec = match decl {
        WorkspaceDef::Local { base } => WorkspaceSpec::Local {
            base: PathBuf::from(base),
        },
        WorkspaceDef::Clone {
            base,
            remote: _,
            excludes,
            includes,
            preserve_mtime,
            apply_back:
                ApplyBackDef {
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
        WorkspaceDef::Sandbox {
            base,
            excludes,
            includes,
            preserve_mtime,
            apply_back:
                ApplyBackDef {
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
    use iter_language::{SandboxNetworkDef, SandboxPolicyDef, WorkspaceDef};

    #[test]
    fn factory_yields_distinct_local_instances() {
        let decl = WorkspaceDef::Local {
            base: "/tmp/iter-cli-test".into(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let a = factory();
        let b = factory();
        // The supply yields trait objects; each carries the workspace-kind
        // label rather than a concrete type the caller can match on.
        assert_eq!(a.name(), "local");
        assert_eq!(b.name(), "local");
    }

    fn sync_apply_back() -> ApplyBackDef {
        ApplyBackDef {
            mode: CloneApplyBackMode::Sync,
            excludes: Vec::new(),
            includes: Vec::new(),
        }
    }

    #[test]
    fn factory_handles_clone_decl() {
        let decl = WorkspaceDef::Clone {
            base: "/tmp/iter-cli-test".into(),
            remote: None,
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert_eq!(w.name(), "clone");
    }

    #[test]
    fn factory_handles_clone_with_remote() {
        let decl = WorkspaceDef::Clone {
            base: "/tmp/iter-cli-test".into(),
            remote: Some("https://example.com/repo".into()),
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert_eq!(w.name(), "clone");
    }

    #[test]
    fn factory_handles_sandbox_decl() {
        let decl = WorkspaceDef::Sandbox {
            base: "/tmp/iter-cli-test".into(),
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
            policy: SandboxPolicyDef {
                network: SandboxNetworkDef::Off,
                allow_read_outside: Vec::new(),
                allow_write_outside: Vec::new(),
                extra_deny_paths: Vec::new(),
                allow_exec: Vec::new(),
            },
        };
        let factory = build_workspace_factory(&decl, SandboxRequirements::default());
        let w = factory();
        assert_eq!(w.name(), "sandbox");
    }
}
