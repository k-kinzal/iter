//! [`SandboxRequirements`] — per-agent declaration of the OS-level access
//! its child process needs to function inside a sandboxed workspace.
//!
//! A [`SandboxWorkspace`](crate::workspace::SandboxWorkspace) implementation
//! is free to build a maximally-restrictive default profile (no network, no
//! reads or writes outside the workspace tmpdir, scrubbed environment).
//! Concrete agents then declare the minimum set of capabilities they
//! actually need; the workspace merges the two to produce the final
//! profile.
//!
//! The fields are deliberately format-agnostic (host names, paths, env-var
//! patterns) so the same declaration can be consumed by a macOS
//! `sandbox-exec` profile, a Linux `bwrap` argv, or any other backend.

use std::path::PathBuf;

/// OS-level access an [`Agent`](crate::agent::Agent) needs to function
/// inside a sandboxed workspace.
///
/// Every field is additive over the workspace's default-deny baseline:
/// an empty [`SandboxRequirements::default`] still works for agents that
/// neither read external state nor hit the network (rare in practice, but
/// the right default for tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxRequirements {
    /// Outbound network endpoints the agent must reach.
    ///
    /// Entries are `host` or `host:port` strings. A bare host implies
    /// any port; backends that cannot filter by port (e.g. some
    /// `sandbox-exec` profiles) treat the port suffix as advisory.
    pub network_hosts: Vec<String>,

    /// Absolute filesystem paths the agent must be able to **read**
    /// outside the workspace tmpdir.
    ///
    /// Typically configuration files, credential stores, or platform
    /// keychain-shim paths. Subdirectories are included recursively.
    pub file_reads: Vec<PathBuf>,

    /// Absolute filesystem paths the agent must be able to **write**
    /// outside the workspace tmpdir.
    ///
    /// Keep this list as small as possible: every entry is a hole in the
    /// sandbox. Agents that write session logs under `~/.claude/` are
    /// the canonical use case.
    pub file_writes: Vec<PathBuf>,

    /// Regex patterns that match additional writable paths the agent
    /// needs — for files whose names carry a per-session random
    /// component that cannot be enumerated ahead of time.
    ///
    /// The canonical case is Claude Code's shell wrapper, which writes
    /// the current working directory to `/tmp/claude-<hex4>-cwd` (a
    /// fresh hex suffix per session); no literal [`file_writes`] entry
    /// can cover an unknown hex, but a regex
    /// `^/private/tmp/claude-[0-9a-f]+-cwd$` can.
    ///
    /// Each entry is an anchored regex in sandbox-exec's SBPL syntax —
    /// the Linux `bwrap` backend does not consume this field (bind
    /// mounts cannot be regex-matched), so agents that rely on it
    /// should also provide a literal `file_writes` fallback when a
    /// predictable path exists.
    ///
    /// [`file_writes`]: Self::file_writes
    pub file_write_regexes: Vec<String>,

    /// Environment-variable name patterns the agent needs propagated
    /// into the sandbox.
    ///
    /// Entries may be exact names (`"ANTHROPIC_API_KEY"`) or suffix
    /// wildcards with a trailing `*` (`"CLAUDE_*"`). The wildcard form
    /// matches any variable whose name begins with the prefix before the
    /// `*`. Backends that cannot introspect the live environment expand
    /// wildcards at sandbox-build time against `std::env::vars()`.
    pub env_pass: Vec<String>,

    /// Whether the agent needs to send signals to processes other than
    /// itself — e.g. `kill`/`killpg` to shut down child commands it
    /// spawned, or `nice`/`setpriority` to re-nice them.
    ///
    /// Default `false` keeps the macOS backend's `(allow signal (target
    /// self))` rule, which permits intra-process signals (abort handlers,
    /// `pthread_kill`) but blocks signalling any other process. Agents that
    /// manage child processes (Claude Code's Bash tool runs shell
    /// pipelines and must SIGTERM them on timeout) set this to `true` so
    /// the backend emits the broader `(allow signal)` instead.
    ///
    /// On the Linux `bwrap` backend this field is a no-op: the PID
    /// namespace created by `--unshare-all` already confines signal
    /// delivery to descendants of the sandboxed root.
    pub allow_signal: bool,
}

impl SandboxRequirements {
    /// Construct an empty requirements set — nothing passes through.
    ///
    /// This is the default any agent inherits automatically; the compose
    /// layer dispatches per concrete agent type and returns an overridden
    /// instance for agents that need extra capabilities.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `name` matches any entry in
    /// [`env_pass`](Self::env_pass), honoring the suffix-wildcard form.
    #[must_use]
    pub fn env_matches(&self, name: &str) -> bool {
        self.env_pass.iter().any(|pat| match_env_pattern(pat, name))
    }
}

/// Expand this module's env-var pattern against a concrete variable name.
///
/// Accepts exact names or suffix-wildcards (`PREFIX_*`). Backends that
/// need to produce the full concrete list of variables (e.g. for a
/// `sandbox-exec` profile that cannot itself match patterns) call this
/// helper for every candidate.
#[must_use]
pub fn match_env_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_matches_exact() {
        let r = SandboxRequirements {
            env_pass: vec!["ANTHROPIC_API_KEY".into()],
            ..Default::default()
        };
        assert!(r.env_matches("ANTHROPIC_API_KEY"));
        assert!(!r.env_matches("ANTHROPIC_OTHER"));
    }

    #[test]
    fn env_matches_prefix_wildcard() {
        let r = SandboxRequirements {
            env_pass: vec!["CLAUDE_*".into()],
            ..Default::default()
        };
        assert!(r.env_matches("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(r.env_matches("CLAUDE_"));
        assert!(!r.env_matches("ANTHROPIC_API_KEY"));
    }
}
