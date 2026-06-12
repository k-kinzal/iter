//! [`SandboxProfile`] — the OS-level access an agent's child process needs
//! to function inside a sandboxed workspace, **assembled by the sandbox
//! layer from the agent's [`AgentKind`]** rather than declared by the agent.
//!
//! # Where the per-agent policy lives
//!
//! The agent reports only individual, object-safe *facts* — its
//! [`kind`](crate::Agent::kind), its [`command_path`](crate::Agent::command_path),
//! and (for a composite) its [`sub_agents`](crate::Agent::sub_agents). The
//! agent holds **no aggregating sandbox type**. The sandbox-shaped policy
//! (which network hosts, which env passthrough patterns, which OS holes for
//! the keychain / shell-tool tmp dirs, whether to broaden signalling) is an
//! environment-shaped concern, so it lives here, keyed off the kind.
//!
//! [`SandboxProfile::for_agent`] performs an **exhaustive `match` over the
//! closed [`AgentKind`]**: adding a new kind without a matching arm is a
//! non-exhaustive-match compile error — the no-omission guarantee. Each arm
//! reaches only for object-safe accessors and per-kind constants; there is
//! no downcast to a concrete driver.
//!
//! The fields are deliberately format-agnostic (host names, paths, env-var
//! patterns) so the same profile can be rendered by a macOS `sandbox-exec`
//! profile, a Linux `bwrap` argv, or any other [backend](super::backend).

use std::path::PathBuf;

use crate::agent::{Agent, AgentKind, ClaudeAgent, GrokAgent};

/// OS-level access an [`Agent`](crate::Agent)'s child process needs to
/// function inside a sandboxed workspace.
///
/// Every field is additive over the workspace's default-deny baseline: an
/// empty [`SandboxProfile::default`] still works for agents that neither
/// read external state nor hit the network (the closed set of CLI drivers
/// with no special needs — Codex, Gemini, … — and the in-process Noop/Fake
/// agents). Profiles are assembled by [`for_agent`](Self::for_agent), never
/// hand-built outside tests; the builder methods exist so the per-kind match
/// arms read as a fine-grained allow-list rather than a struct literal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxProfile {
    /// Outbound network endpoints the agent must reach.
    ///
    /// Entries are `host` or `host:port` strings. A bare host implies any
    /// port; backends that cannot filter by port (e.g. some `sandbox-exec`
    /// profiles) treat the port suffix as advisory.
    pub(crate) network_hosts: Vec<String>,

    /// Absolute filesystem paths the agent must be able to **read** outside
    /// the workspace tmpdir.
    ///
    /// Typically configuration files, credential stores, or platform
    /// keychain-shim paths. Subdirectories are included recursively.
    pub(crate) file_reads: Vec<PathBuf>,

    /// Absolute filesystem paths the agent must be able to **write** outside
    /// the workspace tmpdir.
    ///
    /// Keep this list as small as possible: every entry is a hole in the
    /// sandbox. Agents that write session logs under `~/.claude/` are the
    /// canonical use case.
    pub(crate) file_writes: Vec<PathBuf>,

    /// Regex patterns that match additional writable paths the agent needs —
    /// for files whose names carry a per-session random component that
    /// cannot be enumerated ahead of time.
    ///
    /// The canonical case is Claude Code's shell wrapper, which writes the
    /// current working directory to `/tmp/claude-<hex4>-cwd` (a fresh hex
    /// suffix per session); no literal [`file_writes`] entry can cover an
    /// unknown hex, but a regex `^/private/tmp/claude-[0-9a-f]+-cwd$` can.
    ///
    /// Each entry is an anchored regex in sandbox-exec's SBPL syntax — the
    /// Linux `bwrap` backend does not consume this field (bind mounts cannot
    /// be regex-matched), so arms that rely on it should also provide a
    /// literal `file_writes` fallback when a predictable path exists.
    ///
    /// [`file_writes`]: Self::file_writes
    pub(crate) file_write_regexes: Vec<String>,

    /// Environment-variable name patterns the agent needs propagated into
    /// the sandbox.
    ///
    /// Entries may be exact names (`"ANTHROPIC_API_KEY"`) or suffix wildcards
    /// with a trailing `*` (`"CLAUDE_*"`). The wildcard form matches any
    /// variable whose name begins with the prefix before the `*`. Backends
    /// that cannot introspect the live environment expand wildcards at
    /// sandbox-build time against `std::env::vars()`.
    pub(crate) env_pass: Vec<String>,

    /// Whether the agent needs to send signals to processes other than
    /// itself — e.g. `kill`/`killpg` to shut down child commands it spawned.
    ///
    /// Default `false` keeps the macOS backend's `(allow signal (target
    /// self))` rule, which permits intra-process signals but blocks
    /// signalling any other process. Agents that manage child processes
    /// (Claude Code's Bash tool runs shell pipelines and must SIGTERM them on
    /// timeout) set this so the backend emits the broader `(allow signal)`.
    ///
    /// On the Linux `bwrap` backend this field is a no-op: the PID namespace
    /// created by `--unshare-all` already confines signal delivery to
    /// descendants of the sandboxed root.
    pub(crate) allow_signal: bool,
}

impl SandboxProfile {
    /// Construct an empty profile — nothing passes through.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Assemble the profile for `agent` via an **exhaustive `match` over the
    /// closed [`AgentKind`]**.
    ///
    /// This is the single entry point the sandbox layer uses: the
    /// [`SandboxWorkspace`](super::SandboxWorkspace) is constructed with the
    /// result. Adding an [`AgentKind`] variant without a matching arm is a
    /// compile error, which is exactly the coverage guarantee — the sandbox
    /// can never silently forget an agent.
    ///
    /// The composite [`Router`](AgentKind::Router) arm unions the profiles of
    /// the router's [`sub_agents`](crate::Agent::sub_agents), applying each
    /// sub-agent's arm in turn (recursively, so nested routers compose).
    #[must_use]
    pub fn for_agent(agent: &dyn Agent) -> Self {
        let mut profile = Self::new();
        profile.apply_agent(agent);
        profile.normalize();
        profile
    }

    /// Apply a single agent's per-kind policy to `self`. Recurses through a
    /// router's sub-agents. Kept separate from [`for_agent`](Self::for_agent)
    /// so the union for a [`Router`](AgentKind::Router) can accumulate into
    /// one profile before a final [`normalize`](Self::normalize).
    fn apply_agent(&mut self, agent: &dyn Agent) {
        match agent.kind() {
            AgentKind::Claude => self.apply_claude(agent),
            AgentKind::Grok => self.apply_grok(agent),
            // No special OS access beyond the workspace tmpdir. The
            // remaining CLI drivers either talk only to already-allowed
            // hosts or have not yet had their minimal profile characterised
            // (Codex/Gemini/…); the in-process Noop/Fake agents need
            // nothing. An empty arm is the correct, intentional baseline —
            // and the exhaustive match still forces a decision for each.
            AgentKind::Codex
            | AgentKind::Gemini
            | AgentKind::Hermes
            | AgentKind::Antigravity
            | AgentKind::Copilot
            | AgentKind::Cursor
            | AgentKind::Cline
            | AgentKind::OpenCode
            | AgentKind::Generic
            | AgentKind::Noop
            | AgentKind::Fake => {}
            AgentKind::Router => {
                // Invariant: any agent reporting `AgentKind::Router` MUST
                // override `sub_agents()` (the trait default is empty). A
                // composite that forgot to would silently union nothing here —
                // a profile that is too tight, which fails loud (denied
                // access) rather than silently permissive, but is still wrong.
                for (_name, sub) in agent.sub_agents() {
                    self.apply_agent(sub.as_ref());
                }
            }
        }
    }

    /// Claude Code's sandbox access profile.
    ///
    /// * **Network**: only the inference API (`api.anthropic.com`) and the
    ///   OAuth login host (`console.anthropic.com`). No telemetry, no CDN.
    /// * **Reads**: the auth-critical files under `${HOME}/.claude/`
    ///   (credentials + settings), the legacy `~/.claude.json`, and the
    ///   resolved path of the binary (so the backend can map `claude` when it
    ///   lives outside the default system-binary allow-list). On macOS the
    ///   OAuth token is in the login keychain, so `~/Library/Keychains` is
    ///   added. Read access to `~/.claude/projects/` is deliberately withheld
    ///   so a misbehaving agent cannot exfiltrate other workspaces'
    ///   transcripts.
    /// * **Writes**: `~/.claude` (session state sink) — write-only, never
    ///   readable, preserving Wide & Shallow isolation. On macOS also the
    ///   keychain, the CLI's scratch cache, the `Bash` tool's per-uid tmp
    ///   dir, the `/private/tmp/claude-<hex4>-cwd` shell wrapper file, and the
    ///   canonical `$TMPDIR` (for zsh heredoc bodies).
    /// * **Env**: the vendor wildcards (`CLAUDE_*`, `ANTHROPIC_*`) plus the
    ///   platform-standard variables every CLI expects.
    /// * **Signal**: Claude Code's Bash tool SIGTERMs the shell pipelines it
    ///   spawns when their declared timeout fires.
    fn apply_claude(&mut self, agent: &dyn Agent) {
        self.allow_network_host("api.anthropic.com:443")
            .allow_network_host("console.anthropic.com:443")
            .allow_signal();
        for pat in [
            "CLAUDE_*",
            "ANTHROPIC_*",
            "HOME",
            "USER",
            "LOGNAME",
            "PATH",
            "SHELL",
            "TMPDIR",
            "TERM",
            "LANG",
            "LC_ALL",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            self.pass_env(pat);
        }

        // Auth/config files are individually readable; the `~/.claude`
        // subtree as a whole is write-only (session sink). Reads are NOT
        // granted for the subtree so the agent cannot read back any past
        // iteration's session log.
        for path in [
            ClaudeAgent::credentials_path(),
            ClaudeAgent::settings_path(),
            ClaudeAgent::user_config_path(),
        ]
        .into_iter()
        .flatten()
        {
            self.allow_read(path);
        }
        if let Some(dir) = ClaudeAgent::home_dir() {
            self.allow_write(dir);
        }
        if let Some(path) = ClaudeAgent::user_config_path() {
            self.allow_write(path);
        }

        // macOS-specific state. On macOS the OAuth token lives in the login
        // keychain (not `~/.claude/.credentials.json`), and the CLI stashes
        // MCP transport logs under `~/Library/Caches/claude-cli-nodejs/`.
        #[cfg(target_os = "macos")]
        if let Some(home) = crate::home::home_dir() {
            let library = home.join("Library");
            let keychains = library.join("Keychains");
            let cache = library.join("Caches").join("claude-cli-nodejs");
            self.allow_read(keychains.clone());
            self.allow_write(keychains);
            self.allow_write(cache);
        }

        // The `Bash` tool stages every shell invocation under
        // `/private/tmp/claude-<UID>/…`; both read and write are required.
        #[cfg(target_os = "macos")]
        {
            let dir = ClaudeAgent::bash_tmp_dir();
            self.allow_read(dir.clone());
            self.allow_write(dir);
        }

        // claude-code's zsh wrapper writes the cwd to
        // `/tmp/claude-<hex4>-cwd` (fresh per session, so a regex not a
        // literal), and zsh heredocs stage their bodies under `$TMPDIR`.
        #[cfg(target_os = "macos")]
        {
            self.allow_write_regex(r"^/private/tmp/claude-[0-9a-f]+-cwd$");
            if let Ok(tmpdir) = std::env::var("TMPDIR")
                && let Ok(canonical) = std::fs::canonicalize(&tmpdir)
            {
                self.allow_write(canonical);
            }
        }

        // Resolved binary path (plus canonical target behind a shim) — the
        // backend must read the executable image to map it into the child.
        if let Some(cp) = agent.command_path() {
            self.allow_reads(cp.reads());
        }
    }

    /// Grok Build's sandbox access profile. Mirrors the Claude read/write
    /// split: the auth/config files are individually readable while the
    /// `~/.grok` session-transcript subtree is write-only.
    ///
    /// * **Network**: the xAI inference API host (`api.x.ai`) and the OAuth
    ///   login/refresh host (`auth.x.ai`). Login-mode inference through
    ///   `cli-chat-proxy.grok.com` is widened via the project's upper-bound
    ///   policy, not here.
    /// * **Reads**: `${HOME}/.grok/auth.json` (OAuth token store) and
    ///   `${HOME}/.grok/config.toml` (CLI settings), plus the resolved binary.
    /// * **Writes**: `${HOME}/.grok` — config root and headless session-state
    ///   sink; write-only so cross-workspace transcripts cannot be read back.
    /// * **Env**: the vendor wildcards (`XAI_*`, `GROK_*`) plus the
    ///   platform-standard variables.
    /// * **Signal**: Grok's shell tooling SIGTERMs the pipelines it spawns on
    ///   timeout.
    fn apply_grok(&mut self, agent: &dyn Agent) {
        self.allow_network_host("api.x.ai:443")
            .allow_network_host("auth.x.ai:443")
            .allow_signal();
        for pat in [
            "XAI_*",
            "GROK_*",
            "HOME",
            "USER",
            "LOGNAME",
            "PATH",
            "SHELL",
            "TMPDIR",
            "TERM",
            "LANG",
            "LC_ALL",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            self.pass_env(pat);
        }

        for path in [GrokAgent::auth_path(), GrokAgent::config_path()]
            .into_iter()
            .flatten()
        {
            self.allow_read(path);
        }
        if let Some(dir) = GrokAgent::home_dir() {
            self.allow_write(dir);
        }
        if let Some(cp) = agent.command_path() {
            self.allow_reads(cp.reads());
        }
    }

    /// Sort and deduplicate every allow-list so a router's union of its
    /// sub-agents' profiles is deterministic and free of repeats. `bool`
    /// fields are already idempotent under OR.
    fn normalize(&mut self) {
        self.network_hosts.sort();
        self.network_hosts.dedup();
        self.file_reads.sort();
        self.file_reads.dedup();
        self.file_writes.sort();
        self.file_writes.dedup();
        self.file_write_regexes.sort();
        self.file_write_regexes.dedup();
        self.env_pass.sort();
        self.env_pass.dedup();
    }

    /// Allow outbound access to a network endpoint (`host` or `host:port`).
    pub fn allow_network_host(&mut self, host: impl Into<String>) -> &mut Self {
        self.network_hosts.push(host.into());
        self
    }

    /// Allow reading an absolute path (recursively) outside the workspace.
    pub fn allow_read(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.file_reads.push(path.into());
        self
    }

    /// Allow reading every path the iterator yields.
    pub fn allow_reads(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> &mut Self {
        self.file_reads.extend(paths);
        self
    }

    /// Allow writing an absolute path (recursively) outside the workspace.
    pub fn allow_write(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.file_writes.push(path.into());
        self
    }

    /// Allow writing paths matching an anchored sandbox-exec SBPL regex (for
    /// per-session random names that no literal path can cover).
    pub fn allow_write_regex(&mut self, regex: impl Into<String>) -> &mut Self {
        self.file_write_regexes.push(regex.into());
        self
    }

    /// Propagate environment variables matching `pattern` (an exact name or a
    /// `PREFIX_*` suffix wildcard) into the sandbox.
    pub fn pass_env(&mut self, pattern: impl Into<String>) -> &mut Self {
        self.env_pass.push(pattern.into());
        self
    }

    /// Broaden the signal rule from self-only to any same-UID process.
    pub fn allow_signal(&mut self) -> &mut Self {
        self.allow_signal = true;
        self
    }

    /// Returns `true` if `name` matches any entry in `env_pass`, honoring the
    /// suffix-wildcard form.
    #[must_use]
    pub fn env_matches(&self, name: &str) -> bool {
        self.env_pass.iter().any(|pat| match_env_pattern(pat, name))
    }
}

/// Expand this module's env-var pattern against a concrete variable name.
///
/// Accepts exact names or suffix-wildcards (`PREFIX_*`). Backends that need
/// to produce the full concrete list of variables (e.g. for a `sandbox-exec`
/// profile that cannot itself match patterns) call this helper for every
/// candidate.
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
    use crate::agent::{AgentMode, AgentRouter, RoutingStrategy};
    use std::sync::{Mutex, PoisonError};
    use tempfile::TempDir;

    /// Serializes the tests that mutate the process-global `HOME` / `PATH`.
    /// `for_agent` reads those vars (via the agent's config-dir accessors), so
    /// two env-mutating tests running concurrently would observe each other's
    /// temp values and fail the exact-path assertions. Every test that calls
    /// `set_var` acquires this lock for its whole body.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn claude_agent(command: impl Into<String>) -> ClaudeAgent {
        ClaudeAgent {
            command: command.into(),
            mode: AgentMode::Headless,
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    fn grok_agent(command: impl Into<String>) -> GrokAgent {
        GrokAgent {
            command: command.into(),
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    // ----- env pattern matching (ported from requirements.rs) -------------

    #[test]
    fn env_matches_exact() {
        let mut p = SandboxProfile::new();
        p.pass_env("ANTHROPIC_API_KEY");
        assert!(p.env_matches("ANTHROPIC_API_KEY"));
        assert!(!p.env_matches("ANTHROPIC_OTHER"));
    }

    #[test]
    fn env_matches_prefix_wildcard() {
        let mut p = SandboxProfile::new();
        p.pass_env("CLAUDE_*");
        assert!(p.env_matches("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(p.env_matches("CLAUDE_"));
        assert!(!p.env_matches("ANTHROPIC_API_KEY"));
    }

    // ----- Claude profile (behavior preserved across the refactor) --------

    #[test]
    fn claude_declares_anthropic_hosts_and_auth_reads() {
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        assert!(
            p.network_hosts
                .iter()
                .any(|h| h.starts_with("api.anthropic.com")),
            "missing api.anthropic.com in {:?}",
            p.network_hosts,
        );
        assert!(
            p.network_hosts
                .iter()
                .any(|h| h.starts_with("console.anthropic.com")),
            "missing console.anthropic.com in {:?}",
            p.network_hosts,
        );
        assert!(p.env_matches("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(p.env_matches("ANTHROPIC_API_KEY"));
        assert!(p.env_matches("PATH"));
        assert!(
            p.file_writes.iter().any(|p| p.ends_with(".claude")),
            "writes must include ~/.claude (session state sink), got {:?}",
            p.file_writes,
        );
        #[cfg(target_os = "macos")]
        assert!(
            p.file_writes
                .iter()
                .any(|p| p.ends_with("Library/Keychains")),
            "macOS writes must include Library/Keychains, got {:?}",
            p.file_writes,
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_macos_includes_keychain_and_cli_cache() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored below.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let keychain = home.path().join("Library").join("Keychains");
        let cache = home
            .path()
            .join("Library")
            .join("Caches")
            .join("claude-cli-nodejs");
        assert!(p.file_reads.iter().any(|p| p == &keychain));
        assert!(p.file_writes.iter().any(|p| p == &keychain));
        assert!(p.file_writes.iter().any(|p| p == &cache));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_macos_includes_bash_tool_state_dir() {
        // SAFETY: `getuid` cannot fail and returns a process-global value.
        let uid = unsafe { libc::getuid() };
        let expected = PathBuf::from(format!("/private/tmp/claude-{uid}"));
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        assert!(p.file_writes.iter().any(|p| p == &expected));
        assert!(p.file_reads.iter().any(|p| p == &expected));
        assert!(
            p.file_write_regexes
                .iter()
                .any(|re| re == "^/private/tmp/claude-[0-9a-f]+-cwd$"),
        );
    }

    #[test]
    fn claude_allow_signal_for_bash_tool_timeouts() {
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        assert!(p.allow_signal);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_macos_includes_tmpdir_write_for_zsh_heredoc() {
        let Ok(tmpdir) = std::env::var("TMPDIR") else {
            return;
        };
        let Ok(canonical) = std::fs::canonicalize(&tmpdir) else {
            return;
        };
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        assert!(
            p.file_writes.iter().any(|p| p == &canonical),
            "writes must include canonical $TMPDIR ({canonical:?}), got {:?}",
            p.file_writes,
        );
    }

    #[test]
    fn claude_read_only_auth_files_when_home_set() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored below.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(
            p.file_reads
                .iter()
                .any(|p| p.ends_with(".credentials.json")),
        );
        assert!(p.file_reads.iter().any(|p| p.ends_with("settings.json")));
        assert!(
            !p.file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("projects")),
            "must not expose ~/.claude/projects/, got {:?}",
            p.file_reads,
        );
    }

    #[test]
    fn claude_home_is_writable_but_not_readable() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored below.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let p = SandboxProfile::for_agent(&claude_agent("claude"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let claude_dir = home.path().join(".claude");
        assert!(p.file_writes.iter().any(|p| p == &claude_dir));
        assert!(
            !p.file_reads.iter().any(|p| p == &claude_dir),
            "~/.claude must not be a read subpath, got {:?}",
            p.file_reads,
        );
    }

    #[test]
    fn claude_includes_absolute_command_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin = tmp.path().join("fake-claude");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");
        let p = SandboxProfile::for_agent(&claude_agent(bin.to_string_lossy()));
        assert!(p.file_reads.iter().any(|p| p == &bin));
    }

    #[test]
    fn claude_includes_canonical_target_behind_symlink() {
        let tmp = TempDir::new().expect("tmp");
        let target = tmp.path().join("real-claude");
        std::fs::write(&target, b"#!/bin/sh\nexit 0\n").expect("write target");
        let symlink = tmp.path().join("claude-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &symlink).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(&target, &symlink).expect("copy fallback");

        let p = SandboxProfile::for_agent(&claude_agent(symlink.to_string_lossy()));
        assert!(p.file_reads.iter().any(|p| p == &symlink));
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");
        assert!(p.file_reads.iter().any(|p| p == &canonical));
    }

    #[test]
    fn claude_omits_missing_command() {
        let p = SandboxProfile::for_agent(&claude_agent("/nonexistent/definitely-not/claude-xyz"));
        assert!(
            !p.file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("claude-xyz")),
        );
    }

    #[test]
    fn claude_resolves_name_via_path() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let tmp = TempDir::new().expect("tmp");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin = bin_dir.join("claude-path-lookup-probe");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");

        let saved = std::env::var_os("PATH");
        // SAFETY: tests in this module run serially; PATH restored below.
        unsafe {
            std::env::set_var("PATH", bin_dir.as_os_str());
        }
        let p = SandboxProfile::for_agent(&claude_agent("claude-path-lookup-probe"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(p.file_reads.iter().any(|p| p == &bin));
    }

    // ----- Grok profile (behavior preserved across the refactor) ----------

    #[test]
    fn grok_declares_xai_hosts_and_env_passthrough() {
        let p = SandboxProfile::for_agent(&grok_agent("grok"));
        assert!(p.network_hosts.iter().any(|h| h.starts_with("api.x.ai")));
        assert!(p.network_hosts.iter().any(|h| h.starts_with("auth.x.ai")));
        assert!(p.env_matches("XAI_API_KEY"));
        assert!(p.env_matches("GROK_CONFIG_DIR"));
        assert!(p.env_matches("PATH"));
    }

    #[test]
    fn grok_reads_auth_and_config_files_when_home_set() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored below.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let p = SandboxProfile::for_agent(&grok_agent("grok"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let auth = home.path().join(".grok").join("auth.json");
        let config = home.path().join(".grok").join("config.toml");
        assert!(p.file_reads.iter().any(|p| p == &auth));
        assert!(p.file_reads.iter().any(|p| p == &config));
        assert!(
            !p.file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("sessions")),
        );
    }

    #[test]
    fn grok_home_is_writable_but_not_readable() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored below.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let p = SandboxProfile::for_agent(&grok_agent("grok"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let grok_dir = home.path().join(".grok");
        assert!(p.file_writes.iter().any(|p| p == &grok_dir));
        assert!(!p.file_reads.iter().any(|p| p == &grok_dir));
    }

    #[test]
    fn grok_allow_signal_for_shell_tool_timeouts() {
        let p = SandboxProfile::for_agent(&grok_agent("grok"));
        assert!(p.allow_signal);
    }

    #[test]
    fn grok_includes_absolute_command_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin = tmp.path().join("fake-grok");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");
        let p = SandboxProfile::for_agent(&grok_agent(bin.to_string_lossy()));
        assert!(p.file_reads.iter().any(|p| p == &bin));
    }

    #[test]
    fn grok_omits_missing_command() {
        let p = SandboxProfile::for_agent(&grok_agent("/nonexistent/definitely-not/grok-xyz"));
        assert!(
            !p.file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("grok-xyz")),
        );
    }

    #[test]
    fn grok_resolves_name_via_path() {
        let _env = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let tmp = TempDir::new().expect("tmp");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin = bin_dir.join("grok-path-lookup-probe");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");

        let saved = std::env::var_os("PATH");
        // SAFETY: env-mutating tests in this module serialise via ENV_LOCK;
        // PATH restored before scope exit.
        unsafe {
            std::env::set_var("PATH", bin_dir.as_os_str());
        }
        let p = SandboxProfile::for_agent(&grok_agent("grok-path-lookup-probe"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(
            p.file_reads.iter().any(|p| p == &bin),
            "PATH-resolved command must appear in file_reads, got {:?}",
            p.file_reads,
        );
    }

    // ----- Default-arm agents and Router merge ----------------------------

    #[test]
    fn noop_agent_has_empty_profile() {
        let p = SandboxProfile::for_agent(&crate::agent::NoopAgent);
        assert_eq!(p, SandboxProfile::default());
    }

    /// A router's profile is the union of its sub-agents' profiles, so a
    /// router over `[claude, grok]` requests both backends' hosts. This is
    /// the sandbox-side replacement for the old CLI router-profile merge.
    #[test]
    fn router_unions_claude_and_grok_profiles() {
        let agents: Vec<(String, Box<dyn Agent>)> = vec![
            ("c".into(), Box::new(claude_agent("claude"))),
            ("g".into(), Box::new(grok_agent("grok"))),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback);
        let p = SandboxProfile::for_agent(&router);
        assert!(
            p.network_hosts
                .iter()
                .any(|h| h.starts_with("api.anthropic.com")),
            "merged profile must include claude's host, got {:?}",
            p.network_hosts,
        );
        assert!(
            p.network_hosts.iter().any(|h| h.starts_with("api.x.ai")),
            "merged profile must include grok's host, got {:?}",
            p.network_hosts,
        );
        assert!(p.env_matches("ANTHROPIC_API_KEY"));
        assert!(p.env_matches("XAI_API_KEY"));
        assert!(p.allow_signal);
    }

    /// The `Router` arm recurses, so a router whose sub-agent is itself a
    /// router still unions the nested leaves. Structured as
    /// `Router[ Router[Claude], Claude, Grok ]` so that two Claude leaves
    /// (one nested, one direct) both contribute `api.anthropic.com:443` — the
    /// single end-of-tree `normalize()` must collapse that to one entry, which
    /// is the assertion that genuinely exercises cross-tree dedup.
    #[test]
    fn nested_router_unions_recursively() {
        let inner: Vec<(String, Box<dyn Agent>)> =
            vec![("c".into(), Box::new(claude_agent("claude")))];
        let inner_router = AgentRouter::new(inner, RoutingStrategy::Fallback);

        let outer: Vec<(String, Box<dyn Agent>)> = vec![
            ("nested".into(), Box::new(inner_router)),
            ("c2".into(), Box::new(claude_agent("claude"))),
            ("g".into(), Box::new(grok_agent("grok"))),
        ];
        let outer_router = AgentRouter::new(outer, RoutingStrategy::Fallback);

        let p = SandboxProfile::for_agent(&outer_router);
        // Recursion: the nested Claude's host surfaces through the inner router.
        assert!(
            p.network_hosts
                .iter()
                .any(|h| h.starts_with("api.anthropic.com")),
            "nested claude host must surface through the inner router, got {:?}",
            p.network_hosts,
        );
        // Distinct leaf: grok's host is present too.
        assert!(
            p.network_hosts.iter().any(|h| h.starts_with("api.x.ai")),
            "outer grok host must be present, got {:?}",
            p.network_hosts,
        );
        // Dedup across the tree: the nested Claude and the direct Claude both
        // contribute api.anthropic.com:443; normalize() collapses it to one.
        let anthropic = p
            .network_hosts
            .iter()
            .filter(|h| h.as_str() == "api.anthropic.com:443")
            .count();
        assert_eq!(
            anthropic, 1,
            "duplicate host from two claude leaves must be deduped, got {:?}",
            p.network_hosts,
        );
    }
}
