//! `bwrap` argv rendering split out of [`linux`](super::linux) so tests
//! can assert on the exact argv without needing `bwrap` on PATH.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::match_env_pattern;

use super::super::policy::NetworkAccess;
use super::SandboxDescriptor;

/// Build the full `bwrap` argv prefix for `descriptor`.
///
/// Split out of [`SandboxBackend::prepare`](super::SandboxBackend::prepare)
/// so tests can assert on the exact argv without needing `bwrap` on PATH.
#[must_use]
pub fn render_argv(descriptor: &SandboxDescriptor<'_>) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    argv.push("bwrap".into());
    argv.push("--die-with-parent".into());
    argv.push("--unshare-all".into());

    // Network policy — `--unshare-all` already denies network; opt back
    // in when the policy allows it.
    match &descriptor.policy.network {
        NetworkAccess::Off => {
            // Explicit: keep the unshared net namespace (nothing to add).
        }
        NetworkAccess::All => {
            argv.push("--share-net".into());
        }
        NetworkAccess::Hosts(_) => {
            // bwrap cannot filter by hostname; if the agent declared any
            // hosts, open the shared net namespace and document that the
            // caller is responsible for external firewalling.
            if !descriptor.requirements.network_hosts.is_empty() {
                argv.push("--share-net".into());
            }
            // Otherwise leave net namespace unshared (default-deny).
        }
    }

    // NOTE: `policy.allow_exec` is intentionally not wired here. bwrap
    // has no built-in execve filter, so implementing it would require
    // either selective bind-mounts (tricky: shared library dependencies)
    // or a seccomp filter (adds a dependency). Tracked as a follow-up.

    // Read-only system plumbing. These are the minimum mounts a
    // dynamically-linked CLI needs to start.
    for path in ["/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc"] {
        let p = Path::new(path);
        if p.exists() {
            argv.push("--ro-bind".into());
            argv.push(p.as_os_str().to_owned());
            argv.push(p.as_os_str().to_owned());
        }
    }
    argv.push("--proc".into());
    argv.push("/proc".into());
    argv.push("--dev".into());
    argv.push("/dev".into());
    argv.push("--tmpfs".into());
    argv.push("/tmp".into());

    // Workspace tmpdir — bind at the same path, read-write.
    let ws = descriptor.workspace_path;
    argv.push("--bind".into());
    argv.push(ws.as_os_str().to_owned());
    argv.push(ws.as_os_str().to_owned());

    // Additional read paths from policy + agent requirements.
    let mut readonly_paths: HashSet<PathBuf> = HashSet::new();
    for path in descriptor
        .policy
        .allow_read_outside
        .iter()
        .chain(descriptor.requirements.file_reads.iter())
    {
        if readonly_paths.insert(path.clone()) {
            argv.push("--ro-bind-try".into());
            argv.push(path.as_os_str().to_owned());
            argv.push(path.as_os_str().to_owned());
        }
    }

    // Additional write paths from policy + agent requirements.
    let mut writable_paths: HashSet<PathBuf> = HashSet::new();
    for path in descriptor
        .policy
        .allow_write_outside
        .iter()
        .chain(descriptor.requirements.file_writes.iter())
    {
        if writable_paths.insert(path.clone()) {
            argv.push("--bind-try".into());
            argv.push(path.as_os_str().to_owned());
            argv.push(path.as_os_str().to_owned());
        }
    }

    // Deny overrides — shadow with an empty tmpfs so even an accidental
    // earlier allow can't reach the real path.
    for path in &descriptor.policy.extra_deny_paths {
        argv.push("--tmpfs".into());
        argv.push(path.as_os_str().to_owned());
    }

    // Environment — bwrap inherits nothing when we --clearenv, so
    // re-populate only the variables the agent requires.
    argv.push("--clearenv".into());
    let env_vars = expand_env_pass(&descriptor.requirements.env_pass);
    for (k, v) in env_vars {
        argv.push("--setenv".into());
        argv.push(OsString::from(k));
        argv.push(OsString::from(v));
    }

    // Working directory — chdir into the workspace so commands that
    // inherit a bogus cwd still land in the right tree.
    argv.push("--chdir".into());
    argv.push(ws.as_os_str().to_owned());

    // End-of-options marker so the caller's argv cannot be mistaken
    // for more bwrap flags.
    argv.push("--".into());
    argv
}

/// Materialise the list of concrete env vars to propagate by expanding
/// wildcard patterns against the process' current environment.
fn expand_env_pass(patterns: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (name, value) in std::env::vars() {
        if patterns
            .iter()
            .any(|pat| match_env_pattern(pat, name.as_str()))
            && seen.insert(name.clone())
        {
            out.push((name, value));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::super::policy::{NetworkAccess, SandboxPolicy};
    use super::super::SandboxDescriptor;
    use super::*;
    use crate::SandboxRequirements;

    /// Default-deny test policy: every field empty, network off. The
    /// production codepath has no `deny_all_policy()` because the
    /// project must spell out its posture in the Iterfile; tests use
    /// this helper instead.
    fn deny_all_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkAccess::Off,
            allow_read_outside: Vec::new(),
            allow_write_outside: Vec::new(),
            extra_deny_paths: Vec::new(),
            allow_exec: Vec::new(),
        }
    }

    fn desc<'a>(
        ws: &'a Path,
        policy: &'a SandboxPolicy,
        reqs: &'a SandboxRequirements,
    ) -> SandboxDescriptor<'a> {
        SandboxDescriptor {
            workspace_path: ws,
            policy,
            requirements: reqs,
        }
    }

    fn argv_contains_pair(argv: &[OsString], a: &str, b: &str) -> bool {
        argv.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    fn argv_contains_triple(argv: &[OsString], a: &str, b: &str, c: &str) -> bool {
        argv.windows(3).any(|w| w[0] == a && w[1] == b && w[2] == c)
    }

    #[test]
    fn argv_starts_with_bwrap_and_unshare() {
        let ws = Path::new("/tmp/iter-ws");
        let p = deny_all_policy();
        let r = SandboxRequirements::default();
        let argv = render_argv(&desc(ws, &p, &r));
        assert_eq!(argv[0], "bwrap");
        assert!(argv.iter().any(|a| a == "--unshare-all"));
        assert!(argv.iter().any(|a| a == "--die-with-parent"));
    }

    #[test]
    fn argv_binds_workspace_rw() {
        let ws = Path::new("/tmp/iter-ws");
        let p = deny_all_policy();
        let r = SandboxRequirements::default();
        let argv = render_argv(&desc(ws, &p, &r));
        assert!(argv_contains_triple(
            &argv,
            "--bind",
            "/tmp/iter-ws",
            "/tmp/iter-ws"
        ));
    }

    #[test]
    fn argv_opens_network_only_when_policy_allows() {
        let ws = Path::new("/tmp/ws");
        let r = SandboxRequirements::default();
        let p_off = SandboxPolicy {
            network: NetworkAccess::Off,
            ..deny_all_policy()
        };
        let argv_off = render_argv(&desc(ws, &p_off, &r));
        assert!(!argv_off.iter().any(|a| a == "--share-net"));

        let p_all = SandboxPolicy {
            network: NetworkAccess::All,
            ..deny_all_policy()
        };
        let argv_all = render_argv(&desc(ws, &p_all, &r));
        assert!(argv_all.iter().any(|a| a == "--share-net"));
    }

    #[test]
    fn argv_hosts_policy_requires_agent_declaration() {
        let ws = Path::new("/tmp/ws");
        let p = SandboxPolicy {
            network: NetworkAccess::Hosts(vec!["api.anthropic.com".into()]),
            ..deny_all_policy()
        };
        let empty = SandboxRequirements::default();
        let declared = SandboxRequirements {
            network_hosts: vec!["api.anthropic.com".into()],
            ..Default::default()
        };
        assert!(
            !render_argv(&desc(ws, &p, &empty))
                .iter()
                .any(|a| a == "--share-net")
        );
        assert!(
            render_argv(&desc(ws, &p, &declared))
                .iter()
                .any(|a| a == "--share-net")
        );
    }

    #[test]
    fn argv_mounts_read_paths_readonly() {
        let ws = Path::new("/tmp/ws");
        let p = deny_all_policy();
        let r = SandboxRequirements {
            file_reads: vec![PathBuf::from("/home/me/.claude/settings.json")],
            ..Default::default()
        };
        let argv = render_argv(&desc(ws, &p, &r));
        assert!(argv_contains_triple(
            &argv,
            "--ro-bind-try",
            "/home/me/.claude/settings.json",
            "/home/me/.claude/settings.json"
        ));
    }

    #[test]
    fn argv_mounts_write_paths_readwrite() {
        let ws = Path::new("/tmp/ws");
        let p = deny_all_policy();
        let r = SandboxRequirements {
            file_writes: vec![PathBuf::from("/home/me/.claude/projects")],
            ..Default::default()
        };
        let argv = render_argv(&desc(ws, &p, &r));
        assert!(argv_contains_triple(
            &argv,
            "--bind-try",
            "/home/me/.claude/projects",
            "/home/me/.claude/projects"
        ));
    }

    #[test]
    fn argv_masks_extra_deny_with_tmpfs() {
        let ws = Path::new("/tmp/ws");
        let p = SandboxPolicy {
            extra_deny_paths: vec![PathBuf::from("/home/me/.ssh")],
            ..deny_all_policy()
        };
        let r = SandboxRequirements::default();
        let argv = render_argv(&desc(ws, &p, &r));
        assert!(argv_contains_pair(&argv, "--tmpfs", "/home/me/.ssh"));
    }

    #[test]
    fn argv_clears_env_then_populates_from_patterns() {
        let ws = Path::new("/tmp/ws");
        let p = deny_all_policy();
        // SAFETY: test-only mutation of a process-global; no other
        // threads should race on this key for the duration of the test.
        unsafe {
            std::env::set_var("ITER_TEST_SANDBOX_ENV_FOO", "bar");
        }
        let r = SandboxRequirements {
            env_pass: vec!["ITER_TEST_SANDBOX_ENV_*".into()],
            ..Default::default()
        };
        let argv = render_argv(&desc(ws, &p, &r));
        unsafe {
            std::env::remove_var("ITER_TEST_SANDBOX_ENV_FOO");
        }
        assert!(argv.iter().any(|a| a == "--clearenv"));
        assert!(argv_contains_triple(
            &argv,
            "--setenv",
            "ITER_TEST_SANDBOX_ENV_FOO",
            "bar"
        ));
    }

    #[test]
    fn argv_ends_with_double_dash_sentinel() {
        let ws = Path::new("/tmp/ws");
        let p = deny_all_policy();
        let r = SandboxRequirements::default();
        let argv = render_argv(&desc(ws, &p, &r));
        assert_eq!(argv.last().unwrap(), "--");
    }
}
