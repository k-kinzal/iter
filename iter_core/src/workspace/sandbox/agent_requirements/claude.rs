//! Claude Code [`SandboxRequirements`] builder.

use crate::agent::ClaudeAgent;
use crate::workspace::sandbox::requirements::SandboxRequirements;

/// Build Claude Code's sandbox access profile.
///
/// The profile is deliberately minimal:
/// * **Network**: only the two hostnames the CLI talks to — the inference
///   API (`api.anthropic.com`) and the OAuth login host
///   (`console.anthropic.com`). No telemetry, no asset CDN.
/// * **Reads**: the auth-critical files under `${HOME}/.claude/`
///   (credentials + settings), the legacy `~/.claude.json`, and the
///   resolved path of the agent binary itself (so `sandbox-exec` /
///   `bwrap` can actually locate and map `claude` when it lives outside
///   the default system-binary allow-list — e.g. `~/.local/bin`,
///   volta/nvm/asdf shims, a homebrew cask under a non-standard
///   prefix). On macOS the OAuth token is stored in the login keychain
///   instead of `~/.claude/.credentials.json`, so `~/Library/Keychains`
///   is added there. Read access to `~/.claude/projects/` (cross-project
///   session logs) is deliberately withheld so a misbehaving agent
///   cannot exfiltrate other workspaces' transcripts.
/// * **Writes**: on macOS the keychain (`~/Library/Keychains`) and the
///   CLI's scratch cache (`~/Library/Caches/claude-cli-nodejs`, where
///   Claude Code writes MCP transport logs). Everything else — session
///   transcripts, edits — happens inside the workspace tmpdir the
///   workspace already allows. `~/.claude/projects/` is intentionally
///   NOT writable: if the CLI insists on writing there the backend will
///   surface a permission error, which is louder than silent data leak.
/// * **Env**: the vendor-specific wildcards (`CLAUDE_*`, `ANTHROPIC_*`)
///   plus the platform-standard variables every CLI expects (HOME, PATH,
///   locale, temp, CA certs).
/// * **`allow_signal`**: Claude Code's Bash tool SIGTERMs the shell
///   pipelines it spawns when their declared timeout fires. Without this
///   the `kill(2)` returns EPERM and the pipeline hangs indefinitely.
#[must_use]
pub fn claude(agent: &ClaudeAgent) -> SandboxRequirements {
    let mut reqs = SandboxRequirements {
        network_hosts: vec![
            "api.anthropic.com:443".into(),
            "console.anthropic.com:443".into(),
        ],
        file_reads: Vec::new(),
        file_writes: Vec::new(),
        file_write_regexes: Vec::new(),
        env_pass: vec![
            "CLAUDE_*".into(),
            "ANTHROPIC_*".into(),
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

    if let Some(p) = ClaudeAgent::credentials_path() {
        reqs.file_reads.push(p);
    }
    if let Some(p) = ClaudeAgent::settings_path() {
        reqs.file_reads.push(p);
    }
    if let Some(p) = ClaudeAgent::user_config_path() {
        reqs.file_reads.push(p);
    }

    // Writes to `~/.claude/` are needed for the per-run state the CLI
    // persists: session transcripts under `projects/<hash>/`, todo
    // snapshots under `todos/`, experiment config under `statsig/`, and
    // shell snapshots. `~/.claude.json` itself also gets rewritten by
    // the CLI on config changes.
    //
    // Reads are deliberately NOT granted for the subtree — only the
    // three config files above are readable. This preserves Wide &
    // Shallow isolation: every iteration writes its own session log but
    // cannot read back any past iteration's log, so the agent cannot
    // "remember" previous exploration paths through the filesystem.
    if let Some(d) = ClaudeAgent::home_dir() {
        reqs.file_writes.push(d);
    }
    if let Some(p) = ClaudeAgent::user_config_path() {
        reqs.file_writes.push(p);
    }

    // macOS-specific state. On macOS the OAuth token lives in the login
    // keychain (not in `~/.claude/.credentials.json` — that file only
    // exists on Linux), and the CLI stashes MCP transport logs under
    // `~/Library/Caches/claude-cli-nodejs/<encoded-cwd>/`. Without these
    // entries claude aborts with "Not logged in" at the first keychain
    // read or fails to open its cache file.
    #[cfg(target_os = "macos")]
    if let Some(home) = crate::home::home_dir() {
        let library = home.join("Library");
        let keychains = library.join("Keychains");
        let cache = library.join("Caches").join("claude-cli-nodejs");
        reqs.file_reads.push(keychains.clone());
        reqs.file_writes.push(keychains);
        reqs.file_writes.push(cache);
    }

    // The `Bash` tool uses
    // `/tmp/claude-<UID>/<encoded-cwd>/<session-uuid>/tasks/` to stage
    // every shell invocation. macOS canonicalizes `/tmp` to
    // `/private/tmp`, so the agent exposes the canonical form via
    // `bash_tmp_dir()`. Both read and write are required: without write,
    // the initial `mkdir` fails with EPERM and no command runs. Without
    // read, the command runs but claude surfaces `Exit code 1 <bash
    // output unavailable … EPERM>`, silently turning every Bash call
    // into a no-op result.
    #[cfg(target_os = "macos")]
    {
        let dir = ClaudeAgent::bash_tmp_dir();
        reqs.file_reads.push(dir.clone());
        reqs.file_writes.push(dir);
    }

    // claude-code's zsh wrapper additionally writes the current working
    // directory to `/tmp/claude-<hex4>-cwd` after every `Bash`
    // invocation — a fresh 4-hex-char suffix per session, so no literal
    // subpath can match. Without write access zsh emits `operation not
    // permitted` on every command and propagates exit 1 even when the
    // user's command succeeded, making every Bash result look failed to
    // the agent. Anchor the regex to `/private/tmp` (the canonical form
    // of `/tmp` on macOS) and to a hex suffix so nothing broader leaks
    // through.
    //
    // zsh's heredoc implementation (used by every `<<EOF ... EOF` block
    // in a `Bash` tool invocation) writes the body to a temp file at
    // `$TMPPREFIX<pid>`, which defaults to `$TMPDIR/zsh<pid>` when
    // `TMPDIR` is set (always the case on macOS — launchd exports a
    // per-user `/var/folders/.../T`). Grant write on the canonical
    // `$TMPDIR` so every temp-file pattern zsh can produce lands in an
    // allowed location.
    #[cfg(target_os = "macos")]
    {
        reqs.file_write_regexes
            .push(r"^/private/tmp/claude-[0-9a-f]+-cwd$".to_string());
        if let Ok(tmpdir) = std::env::var("TMPDIR")
            && let Ok(canonical) = std::fs::canonicalize(&tmpdir)
        {
            reqs.file_writes.push(canonical);
        }
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
    use crate::agent::AgentMode;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn agent(command: impl Into<String>) -> ClaudeAgent {
        ClaudeAgent {
            command: command.into(),
            mode: AgentMode::Headless,
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    #[test]
    fn declares_anthropic_hosts_and_auth_reads() {
        let reqs = claude(&agent("claude"));
        assert!(
            reqs.network_hosts
                .iter()
                .any(|h| h.starts_with("api.anthropic.com")),
            "missing api.anthropic.com in {:?}",
            reqs.network_hosts,
        );
        assert!(
            reqs.network_hosts
                .iter()
                .any(|h| h.starts_with("console.anthropic.com")),
            "missing console.anthropic.com in {:?}",
            reqs.network_hosts,
        );
        assert!(reqs.env_matches("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(reqs.env_matches("ANTHROPIC_API_KEY"));
        assert!(reqs.env_matches("PATH"));

        assert!(
            reqs.file_writes.iter().any(|p| p.ends_with(".claude")),
            "writes must include ~/.claude (session state sink), got {:?}",
            reqs.file_writes,
        );
        #[cfg(target_os = "macos")]
        assert!(
            reqs.file_writes
                .iter()
                .any(|p| p.ends_with("Library/Keychains")),
            "macOS writes must include Library/Keychains (OAuth token store), got {:?}",
            reqs.file_writes,
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_includes_keychain_and_cli_cache() {
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests run serially in this module; HOME restored before
        // scope exit.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let reqs = claude(&agent("claude"));
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
        assert!(
            reqs.file_reads.iter().any(|p| p == &keychain),
            "reads must include Library/Keychains, got {:?}",
            reqs.file_reads,
        );
        assert!(
            reqs.file_writes.iter().any(|p| p == &keychain),
            "writes must include Library/Keychains (token refresh), got {:?}",
            reqs.file_writes,
        );
        assert!(
            reqs.file_writes.iter().any(|p| p == &cache),
            "writes must include Library/Caches/claude-cli-nodejs (MCP logs), got {:?}",
            reqs.file_writes,
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_includes_bash_tool_state_dir() {
        // SAFETY: `getuid` cannot fail and returns a process-global value.
        let uid = unsafe { libc::getuid() };
        let expected = PathBuf::from(format!("/private/tmp/claude-{uid}"));
        let reqs = claude(&agent("claude"));
        assert!(
            reqs.file_writes.iter().any(|p| p == &expected),
            "writes must include /private/tmp/claude-<UID> so claude-code's \
             `Bash` tool can mkdir its per-cwd shell state dir, got {:?}",
            reqs.file_writes,
        );
        assert!(
            reqs.file_reads.iter().any(|p| p == &expected),
            "reads must include /private/tmp/claude-<UID> so claude-code can \
             read back the captured `<task-id>.output` after each Bash call, \
             got {:?}",
            reqs.file_reads,
        );
        assert!(
            reqs.file_write_regexes
                .iter()
                .any(|re| re == "^/private/tmp/claude-[0-9a-f]+-cwd$"),
            "file_write_regexes must include the claude-<hex4>-cwd pattern \
             so zsh's per-Bash cwd redirect does not return non-zero, \
             got {:?}",
            reqs.file_write_regexes,
        );
    }

    #[test]
    fn allow_signal_for_bash_tool_timeouts() {
        let reqs = claude(&agent("claude"));
        assert!(
            reqs.allow_signal,
            "ClaudeAgent must opt into allow_signal so its Bash tool can \
             SIGTERM spawned shell pipelines on timeout",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_includes_tmpdir_write_for_zsh_heredoc() {
        let Ok(tmpdir) = std::env::var("TMPDIR") else {
            return;
        };
        let Ok(canonical) = std::fs::canonicalize(&tmpdir) else {
            return;
        };
        let reqs = claude(&agent("claude"));
        assert!(
            reqs.file_writes.iter().any(|p| p == &canonical),
            "writes must include canonical $TMPDIR ({canonical:?}) so zsh \
             heredocs inside Bash tool calls can stage their bodies, got {:?}",
            reqs.file_writes,
        );
    }

    #[test]
    fn read_only_auth_files_when_home_set() {
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests run serially in this module; HOME restored before
        // scope exit.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let reqs = claude(&agent("claude"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(
            reqs.file_reads
                .iter()
                .any(|p| p.ends_with(".credentials.json")),
            "reads should include credentials.json, got {:?}",
            reqs.file_reads,
        );
        assert!(
            reqs.file_reads.iter().any(|p| p.ends_with("settings.json")),
            "reads should include settings.json, got {:?}",
            reqs.file_reads,
        );
        assert!(
            !reqs
                .file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("projects")),
            "must not expose ~/.claude/projects/ — would leak cross-project logs, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn claude_home_is_writable_but_not_readable() {
        let home = TempDir::new().expect("tmp home");
        let saved = std::env::var_os("HOME");
        // SAFETY: tests in this module run serially; HOME is restored
        // before scope exit.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let reqs = claude(&agent("claude"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        let claude_dir = home.path().join(".claude");
        assert!(
            reqs.file_writes.iter().any(|p| p == &claude_dir),
            "writes must include ~/.claude for session state, got {:?}",
            reqs.file_writes,
        );
        assert!(
            !reqs.file_reads.iter().any(|p| p == &claude_dir),
            "~/.claude must not be a read subpath — would leak session logs, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn includes_absolute_command_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin = tmp.path().join("fake-claude");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");
        let reqs = claude(&agent(bin.to_string_lossy()));
        assert!(
            reqs.file_reads.iter().any(|p| p == &bin),
            "absolute command path must appear in file_reads, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn includes_canonical_target_behind_symlink() {
        let tmp = TempDir::new().expect("tmp");
        let target = tmp.path().join("real-claude");
        std::fs::write(&target, b"#!/bin/sh\nexit 0\n").expect("write target");
        let symlink = tmp.path().join("claude-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &symlink).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(&target, &symlink).expect("copy fallback");

        let reqs = claude(&agent(symlink.to_string_lossy()));
        assert!(
            reqs.file_reads.iter().any(|p| p == &symlink),
            "symlink path must appear in file_reads, got {:?}",
            reqs.file_reads,
        );
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");
        assert!(
            reqs.file_reads.iter().any(|p| p == &canonical),
            "canonical target must appear in file_reads, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn omits_missing_command() {
        let reqs = claude(&agent("/nonexistent/definitely-not-here/claude-xyz"));
        assert!(
            !reqs
                .file_reads
                .iter()
                .any(|p| p.to_string_lossy().contains("claude-xyz")),
            "missing binary must not be added to file_reads, got {:?}",
            reqs.file_reads,
        );
    }

    #[test]
    fn resolves_name_via_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin = bin_dir.join("claude-path-lookup-probe");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");

        let saved = std::env::var_os("PATH");
        // SAFETY: tests in this module run serially (shared HOME above
        // depends on it); PATH is restored before function exit.
        unsafe {
            std::env::set_var("PATH", bin_dir.as_os_str());
        }
        let reqs = claude(&agent("claude-path-lookup-probe"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(
            reqs.file_reads.iter().any(|p| p == &bin),
            "PATH-resolved command must appear in file_reads, got {:?}",
            reqs.file_reads,
        );
    }
}
