//! Definition → workspace translation (`workspace_from_def`).
//!
//! Translates a [`WorkspaceDef`] into the single `Box<dyn Workspace>` the
//! [`Runner`](iter_core::Runner) holds for the whole exploration. Each
//! iteration brackets the instance with
//! [`Workspace::setup`](iter_core::Workspace::setup) →
//! [`ActiveWorkspace::teardown`](iter_core::ActiveWorkspace::teardown).
//!
//! The runtime workspace axis is a trait object (R18): the closed set of
//! workspace kinds lives at the definition layer ([`WorkspaceDef`]); at run
//! time the runner only needs "something that sets up". There is no run-time
//! enum wrapper.

use std::path::PathBuf;

use iter_core::workspace::{
    ApplyBackMode, CloneSettings, CloneWorkspace, LocalWorkspace, SandboxPolicy, SandboxWorkspace,
};
use iter_core::{SandboxProfile, Workspace};
use iter_language::{ApplyBackDef, CloneApplyBackMode, WorkspaceDef};

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

/// Build the runner's workspace from a [`WorkspaceDef`].
///
/// A pure selection-by-variant followed by a mechanical field move: every
/// project-shaped knob flows straight from the declaration (the AST already
/// enforces explicit values — iter ships no project-shaped defaults).
///
/// `profile` is the agent's lower bound (the OS access its drivers need to
/// function), derived at agent-assembly time by
/// [`SandboxProfile::for_drivers`](iter_core::SandboxProfile::for_drivers)
/// and carried into a [`SandboxWorkspace`](iter_core::workspace::SandboxWorkspace).
/// The workspace policy (the project's upper bound) comes from the DSL; for
/// non-sandbox workspaces the parameter is unused.
///
/// Setup-time validation is deferred to
/// [`Workspace::setup`](iter_core::Workspace::setup) on the produced
/// workspace, which is why this function is infallible.
pub(crate) fn workspace_from_def(
    decl: &WorkspaceDef,
    profile: SandboxProfile,
) -> Box<dyn Workspace> {
    match decl {
        WorkspaceDef::Local { base, .. } => Box::new(LocalWorkspace::new(PathBuf::from(base))),
        WorkspaceDef::Clone {
            base,
            source: _,
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
        } => Box::new(CloneWorkspace::new(
            PathBuf::from(base),
            CloneSettings {
                excludes: excludes.clone(),
                includes: includes.clone(),
                preserve_mtime: *preserve_mtime,
                apply_back: map_apply_back_mode(*mode),
                apply_back_excludes: ab_excludes.clone(),
                apply_back_includes: ab_includes.clone(),
            },
        )),
        WorkspaceDef::Sandbox {
            base,
            source: _,
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
        } => Box::new(SandboxWorkspace::new(
            PathBuf::from(base),
            CloneSettings {
                excludes: excludes.clone(),
                includes: includes.clone(),
                preserve_mtime: *preserve_mtime,
                apply_back: map_apply_back_mode(*mode),
                apply_back_excludes: ab_excludes.clone(),
                apply_back_includes: ab_includes.clone(),
            },
            map_sandbox_policy(policy),
            profile,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_language::{SandboxNetworkDef, SandboxPolicyDef, WorkspaceDef};

    #[test]
    fn workspace_from_def_handles_local_decl() {
        let decl = WorkspaceDef::Local {
            base: "/tmp/iter-cli-test".into(),
            source: None,
        };
        let w = workspace_from_def(&decl, SandboxProfile::default());
        // The translation yields a trait object; it carries the
        // workspace-kind label rather than a concrete type to match on.
        assert_eq!(w.name(), "local");
    }

    fn sync_apply_back() -> ApplyBackDef {
        ApplyBackDef {
            mode: CloneApplyBackMode::Sync,
            excludes: Vec::new(),
            includes: Vec::new(),
        }
    }

    #[test]
    fn workspace_from_def_handles_clone_decl() {
        let decl = WorkspaceDef::Clone {
            base: "/tmp/iter-cli-test".into(),
            source: None,
            remote: None,
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let w = workspace_from_def(&decl, SandboxProfile::default());
        assert_eq!(w.name(), "clone");
    }

    #[test]
    fn workspace_from_def_handles_clone_with_remote() {
        let decl = WorkspaceDef::Clone {
            base: "/tmp/iter-cli-test".into(),
            source: None,
            remote: Some("https://example.com/repo".into()),
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: sync_apply_back(),
        };
        let w = workspace_from_def(&decl, SandboxProfile::default());
        assert_eq!(w.name(), "clone");
    }

    #[test]
    fn workspace_from_def_handles_sandbox_decl() {
        let decl = WorkspaceDef::Sandbox {
            base: "/tmp/iter-cli-test".into(),
            source: None,
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
        let w = workspace_from_def(&decl, SandboxProfile::default());
        assert_eq!(w.name(), "sandbox");
    }
}
