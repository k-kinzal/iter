//! Grok Build [`SandboxRequirements`] builder.

use crate::agent::GrokAgent;
use crate::workspace::sandbox::requirements::SandboxRequirements;

/// Build Grok Build's sandbox access profile.
///
/// Mirrors the Claude profile's read/write split: the auth-critical config
/// files are individually readable, while the session-transcript subtree is
/// write-only.
///
/// * **Network**: the xAI inference API host (`api.x.ai`, used with
///   `XAI_API_KEY`) and the OAuth login/token-refresh host (`auth.x.ai`,
///   used by `grok login`) — the analogue of Claude's inference +
///   `console.anthropic.com` login pair. No telemetry, no asset CDN.
///   Login-mode *inference* additionally routes through
///   `cli-chat-proxy.grok.com`; operators relying on that path (rather than
///   an `XAI_API_KEY`) widen the allow-list through the project's
///   upper-bound policy.
/// * **Reads**: `${HOME}/.grok/auth.json` (the on-disk OAuth token store —
///   analogue of Claude's `.credentials.json`) and `${HOME}/.grok/config.toml`
///   (CLI settings — analogue of `settings.json`), plus the resolved path
///   of the agent binary itself (so `sandbox-exec` / `bwrap` can locate and
///   map `grok` when it lives outside the default system-binary allow-list
///   — volta/nvm/asdf shims, a homebrew prefix, `~/.local/bin`).
/// * **Writes**: `${HOME}/.grok` — the CLI's config root and the headless
///   session-state sink under `sessions/`. Without write access there
///   `-s/--session-id` cannot create or update a session, so
///   continuous-context explorations would fail. Read access to the *whole*
///   subtree is deliberately withheld so a misbehaving agent cannot
///   exfiltrate other workspaces' session transcripts — the same Wide &
///   Shallow isolation Claude applies to `~/.claude/projects/`. (Like
///   Claude, that means cross-iteration *filesystem* resumption is bounded
///   under the sandbox; the narrowest continuous-context mode is intended
///   for `local`/`clone` workspaces.)
/// * **Env**: the vendor-specific wildcards (`XAI_*`, `GROK_*`) plus the
///   platform-standard variables every CLI expects (HOME, PATH, locale,
///   temp, CA certs). Headless auth expects `XAI_API_KEY` here unless the
///   operator has a prior `grok login` (then `auth.json` above is read).
/// * **`allow_signal`**: Grok's shell tooling SIGTERMs the pipelines it
///   spawns when their declared timeout fires; without this the `kill(2)`
///   returns EPERM and the pipeline hangs indefinitely.
#[must_use]
pub fn grok(agent: &GrokAgent) -> SandboxRequirements {
    let mut reqs = SandboxRequirements {
        network_hosts: vec!["api.x.ai:443".into(), "auth.x.ai:443".into()],
        file_reads: Vec::new(),
        file_writes: Vec::new(),
        file_write_regexes: Vec::new(),
        env_pass: vec![
            "XAI_*".into(),
            "GROK_*".into(),
            "HOME".into(),
            "USER".into(),
            "LOGNAME".into(),
            "PATH".into(),
            "SHELL".into(),
            "TMPDIR".into(),
            "TERM".into(),
            "LANG".into(),
            "LC_ALL".into(),
            "NODE_EXTRA_CA_CERTS".into(),
            "SSL_CERT_FILE".into(),
            "SSL_CERT_DIR".into(),
        ],
        allow_signal: true,
    };

    // Auth/config files are individually readable; the `~/.grok` subtree as
    // a whole is write-only (session sink) — see the read/write split above.
    if let Some(p) = GrokAgent::auth_path() {
        reqs.file_reads.push(p);
    }
    if let Some(p) = GrokAgent::config_path() {
        reqs.file_reads.push(p);
    }
    if let Some(d) = GrokAgent::home_dir() {
        reqs.file_writes.push(d);
    }

    if let Some(cp) = agent.command_path() {
        // Resolved path (may be a symlink shim) plus canonical target —
        // sandbox-exec and bwrap both need access to the real file to
        // actually map the executable image into the child process.
        reqs.file_reads.extend(cp.reads());
    }

    reqs
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn agent(command: impl Into<String>) -> GrokAgent {
        GrokAgent {
            command: command.into(),
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    #[test]
    fn declares_xai_hosts_and_env_passthrough() {
        let reqs = grok(&agent("grok"));
        assert!(
            reqs.network_hosts.iter().any(|h| h.starts_with("api.x.ai")),
            "missing api.x.ai (inference) in {:?}",
            reqs.network_hosts,
        );
        assert!(
            reqs.network_hosts
                .iter()
                .any(|h| h.starts_with("auth.x.ai")),
            "missing auth.x.ai (OAuth login/refresh) in {:?}",
            reqs.network_hosts,
        );
        assert!(reqs.env_matches("XAI_API_KEY"));
        assert!(reqs.env_matches("GROK_CONFIG_DIR"));
        assert!(reqs.env_matches("PATH"));
    }

    #[test]
    fn reads_auth_and_config_files_when_home_set() {
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME restored before
        // scope exit.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let reqs = grok(&agent("grok"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let auth = home.path().join(".grok").join("auth.json");
        let config = home.path().join(".grok").join("config.toml");
        assert!(
            reqs.file_reads.iter().any(|p| p == &auth),
            "reads must include ~/.grok/auth.json (OAuth token store), got {:?}",
            reqs.file_reads,
        );
        assert!(
            reqs.file_reads.iter().any(|p| p == &config),
            "reads must include ~/.grok/config.toml (CLI settings), got {:?}",
            reqs.file_reads,
        );
        assert!(
            !reqs
                .file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("sessions")),
            "must not expose ~/.grok/sessions/ — would leak cross-workspace transcripts, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn grok_home_is_writable_but_not_readable() {
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME is restored
        // before scope exit.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let reqs = grok(&agent("grok"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        let grok_dir = home.path().join(".grok");
        assert!(
            reqs.file_writes.iter().any(|p| p == &grok_dir),
            "writes must include ~/.grok for headless session state, got {:?}",
            reqs.file_writes,
        );
        assert!(
            !reqs.file_reads.iter().any(|p| p == &grok_dir),
            "~/.grok must not be a read subpath — would leak session logs, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn allow_signal_for_shell_tool_timeouts() {
        let reqs = grok(&agent("grok"));
        assert!(
            reqs.allow_signal,
            "GrokAgent must opt into allow_signal so its shell tooling can \
             SIGTERM spawned pipelines on timeout",
        );
    }

    #[test]
    fn includes_absolute_command_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin = tmp.path().join("fake-grok");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");
        let reqs = grok(&agent(bin.to_string_lossy()));
        assert!(
            reqs.file_reads.iter().any(|p| p == &bin),
            "absolute command path must appear in file_reads, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn omits_missing_command() {
        let reqs = grok(&agent("/nonexistent/definitely-not-here/grok-xyz"));
        assert!(
            !reqs
                .file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("grok-xyz")),
            "missing binary must not be added to file_reads, got {:?}",
            reqs.file_reads,
        );
    }
}
