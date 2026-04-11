//! [`ClaudeAgent`] — Claude Code CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — spawns:
//!
//!   ```text
//!   claude --print [args...]
//!   ```
//!
//!   …with the prompt written to stdin. `--print` tells Claude Code to emit
//!   a single response to stdout and exit — a clean, observable shape for
//!   the `AgentFinished` event payload. No tty required; works in CI and
//!   detached instances.
//!
//! * [`AgentMode::Interactive`] — launches `claude` as a live TUI session
//!   with a project-local Stop hook installed under `${cwd}/.claude/`. The
//!   hook is a direct descendant of
//!   [`agent-loop/claude-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) already
//!   handles signal-level iteration, so the hook only needs to capture the
//!   final assistant message and let `claude` exit cleanly. On return the
//!   agent parses the captured transcript in Rust and populates
//!   [`AgentReport::last_output`](crate::AgentReport) with the last
//!   assistant text message, matching the shape of print mode so event
//!   sinks see the same output surface across modes.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.claude/`. iter never writes to `~/.claude/` because
//!   doing so would silently affect every other Claude Code session on
//!   the machine. See the `hook` submodule for the filesystem layout.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent process
//!   so `claude`'s TUI renders correctly when iter is invoked from a
//!   terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Print`] instead.
//!
//! # Construction
//!
//! [`ClaudeAgent`] exposes no defaults. Every field on [`ClaudeSettings`]
//! is required because the value is a project-shaped decision (binary
//! location, run mode, extra flags) iter cannot honestly pick on behalf
//! of the operator. Call [`ClaudeAgent::new`] with a fully-populated
//! settings struct. See [`ClaudeSettings`] for field-by-field semantics.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod hook;
mod session;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, drive_interactive_with_finalize, inject_agent_otel_resource_attrs,
    inject_trace_context_env, run_command,
};
use hook::HookBundle;
use session::SessionIdFile;

/// Fully-specified configuration for [`ClaudeAgent`].
///
/// Every field is required; there is no `Default` impl because every value
/// is a project-shaped decision the Iterfile must spell out explicitly.
#[derive(Debug, Clone)]
pub struct ClaudeSettings {
    /// Binary name or absolute path passed to
    /// [`tokio::process::Command::new`]. Name-only strings are resolved
    /// via `PATH`.
    pub command: String,
    /// Print vs. interactive mode. See [`AgentMode`].
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags. Empty is
    /// allowed and the common case; omit the knob entirely is not —
    /// the field is required so callers always mean it.
    pub args: Vec<String>,
    /// Optional path (relative to the workspace cwd, unless absolute) of a
    /// file that stores a stable Claude Code session id across iterations.
    /// Genuinely optional: `None` means "no session persistence", `Some`
    /// means "persist the session id at this path".
    pub session_id_file: Option<PathBuf>,
}

/// Claude Code agent configuration.
#[derive(Debug, Clone)]
pub struct ClaudeAgent {
    /// Binary name or path. Required (no implicit `"claude"` fallback).
    pub command: String,
    /// Print vs. interactive mode. Required (no implicit fallback).
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags. Useful for
    /// overriding assumptions like `--model` or `--output-format`.
    pub args: Vec<String>,
    /// Optional path (relative to the workspace cwd, unless absolute) of a
    /// file that stores a stable Claude Code session id across iterations.
    ///
    /// When set, every invocation passes `--session-id <uuid>`:
    ///
    /// * If the file does not exist (or is empty), iter generates a fresh
    ///   v4 UUID, writes it to the path, and hands it to Claude. The `-id`
    ///   flag tells Claude Code to *create* a new session with that id.
    /// * On every subsequent invocation iter reads the same file and
    ///   passes the same uuid, which tells Claude Code to *resume* the
    ///   existing session — giving the agent continuous context across
    ///   iter iterations. This is the narrowest exploration mode because
    ///   accumulated agent context keeps later turns close to earlier ones.
    ///
    /// Lifecycle (deleting the file to end an exploration run) is left to
    /// the caller — typically an `on workspace_teardown_finished` hook that drops
    /// the file on the final iteration. iter does not own that decision
    /// because it has no notion of "end of exploration".
    pub session_id_file: Option<PathBuf>,
}

fn home_subpath(leaf: &str) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(leaf))
}

impl ClaudeAgent {
    /// Build a fully-specified Claude Code agent. Every knob must be
    /// decided by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: ClaudeSettings) -> Self {
        let ClaudeSettings {
            command,
            mode,
            args,
            session_id_file,
        } = settings;
        Self {
            command,
            mode,
            args,
            session_id_file,
        }
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    ///
    /// The returned handle exposes both the resolved path and its
    /// canonical target for sandbox-layer consumers that need to grant
    /// read access to a symlink shim (volta, nvm, asdf, homebrew cask).
    #[must_use]
    pub fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }

    /// `${HOME}/.claude` — persistent configuration root and per-session
    /// state sink (transcripts under `projects/`, todos, statsig, shell
    /// snapshots). `None` when `HOME` is unset.
    #[must_use]
    pub fn home_dir() -> Option<PathBuf> {
        home_subpath(".claude")
    }

    /// `${HOME}/.claude/.credentials.json` — Linux OAuth token store.
    /// macOS keeps the token in the login keychain instead; callers that
    /// need keychain access should combine this with the platform-specific
    /// keychain path exposed by the workspace sandbox layer.
    #[must_use]
    pub fn credentials_path() -> Option<PathBuf> {
        Self::home_dir().map(|d| d.join(".credentials.json"))
    }

    /// `${HOME}/.claude/settings.json` — Claude Code settings file.
    #[must_use]
    pub fn settings_path() -> Option<PathBuf> {
        Self::home_dir().map(|d| d.join("settings.json"))
    }

    /// `${HOME}/.claude.json` — legacy top-level config file the CLI
    /// rewrites on config changes. Distinct from the `.claude/` directory.
    #[must_use]
    pub fn user_config_path() -> Option<PathBuf> {
        home_subpath(".claude.json")
    }

    /// Directory the `Bash` tool uses to stage every shell invocation's
    /// output. macOS canonicalizes `/tmp` to `/private/tmp`, so the path
    /// is emitted in the canonical form the OS will actually check against.
    /// Defined only on macOS — Linux needs nothing outside the workspace.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn bash_tmp_dir() -> PathBuf {
        // SAFETY: `getuid` is always safe — it reads a process-global
        // integer and cannot fail.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/private/tmp/claude-{uid}"))
    }

    fn build_print_command(&self, path: &Path, session_id: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("--print");
        cmd.arg("--permission-mode").arg("bypassPermissions");
        if let Some(sid) = session_id {
            cmd.arg("--session-id").arg(sid);
        }
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    /// Build the interactive-mode command. Passes the prompt as the first
    /// positional argument so `claude` seeds its initial user turn with it
    /// before dropping into the TUI. Extra args come afterward so users
    /// can still inject `--model`, `-c`, and friends.
    ///
    /// `--permission-mode bypassPermissions` is emitted before the extra
    /// args so users can still override it downstream. iter always runs
    /// Claude inside a `sandbox-exec` / `bwrap` profile that is the real
    /// filesystem boundary; the CLI's own per-tool prompt is redundant
    /// and, crucially, cannot be answered from a detached runner — every
    /// `Write`/`Edit` would otherwise silently auto-deny and the agent
    /// would loop reporting "blocked on permissions".
    fn build_interactive_command(
        &self,
        path: &Path,
        prompt: &Prompt,
        session_id: Option<&str>,
    ) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("--permission-mode").arg("bypassPermissions");
        if let Some(sid) = session_id {
            cmd.arg("--session-id").arg(sid);
        }
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for ClaudeAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            ..
        } = ctx;
        // Resolve the session id *before* spawning so a filesystem failure
        // here surfaces as an `AgentError` instead of a confusing child
        // startup error. When `session_id_file` is unset this is a no-op
        // and no `--session-id` flag is emitted, matching the historical
        // behaviour.
        let session_id = match &self.session_id_file {
            Some(file) => Some(
                SessionIdFile::new(file.clone())
                    .resolve(workspace_path)
                    .await?,
            ),
            None => None,
        };
        match self.mode {
            AgentMode::Print => {
                let mut command = self.build_print_command(workspace_path, session_id.as_deref());
                inject_agent_otel_resource_attrs(
                    &mut command,
                    signal_id,
                    signal_kind,
                    workspace_path,
                    "claude",
                );
                if inject_trace_context_env(&mut command) {
                    // Claude Code only consumes env-carried trace context
                    // when telemetry is explicitly enabled for print/SDK runs.
                    command.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1");
                }
                run_command(
                    command,
                    PromptDelivery::Stdin(prompt.as_str()),
                    cancel,
                    stdio_sink,
                )
                .await
            }
            AgentMode::Interactive => {
                self.run_interactive(
                    workspace_path,
                    prompt,
                    session_id.as_deref(),
                    cancel,
                    signal_id,
                    signal_kind,
                )
                .await
            }
        }
    }
}

impl ClaudeAgent {
    /// Drive `claude` as a TUI session. Installs the project-local Stop
    /// hook bundle before spawning and finalizes it after — even on error
    /// paths — so the user's original settings are always restored.
    ///
    /// The run-then-finalize skeleton lives in
    /// [`drive_interactive_with_finalize`]; this method only handles the
    /// Claude-specific bits: bundle install, command construction, and
    /// stdio inheritance wiring.
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        session_id: Option<&str>,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
    ) -> Result<AgentReport, AgentError> {
        // Install the hook bundle. Any failure here is fatal: we cannot
        // safely run without the hook because the interactive TUI would
        // never terminate.
        let bundle = HookBundle::install(path).await?;

        // Build the command and let claude's TUI own stdin/stdout/stderr.
        // We fork an io::inherit() instead of piping because claude TUI
        // renders to a terminal and refuses to start without one.
        let mut command = self.build_interactive_command(path, prompt, session_id);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "claude");
        let (env_key, state_file) = bundle.env_var();
        command.env(env_key, state_file);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        drive_interactive_with_finalize(command, cancel, bundle.finalize()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>, mode: AgentMode) -> ClaudeSettings {
        ClaudeSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            session_id_file: None,
        }
    }

    #[tokio::test]
    async fn print_mode_passes_through_flag_and_stdin() {
        // Fake "claude" binary: records its argv in stdout, then cats stdin.
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'; cat",
        );
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-claude");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("args: --print"), "got {out:?}");
        assert!(out.contains("hello-claude"), "got {out:?}");
    }

    #[tokio::test]
    async fn print_mode_emits_bypass_permissions_flag() {
        let (_guard, bin) = fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(
            out.contains("--permission-mode bypassPermissions"),
            "print mode must emit --permission-mode bypassPermissions, got {out:?}",
        );
    }

    #[tokio::test]
    async fn print_mode_extra_args_are_forwarded_after_print_flag() {
        let (_guard, bin) = fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "opus".into()];
        let agent = ClaudeAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(out.contains("--print"), "got {out:?}");
        assert!(out.contains("--model"), "got {out:?}");
        assert!(out.contains("opus"), "got {out:?}");
    }

    /// Fake `claude` binary for interactive mode.
    ///
    /// Writes a single-message Claude Code transcript to a well-known
    /// path in its cwd, then invokes the installed Stop hook with a
    /// payload that references that transcript. This drives the real
    /// [`HookBundle::finalize`] path end-to-end without needing a tty or
    /// the actual `claude` binary. `$ITER_STATE_FILE` is propagated from
    /// `ClaudeAgent::run_interactive` through the process env so the hook
    /// writes to the exact file the Rust side will read.
    ///
    /// The literal `ITER-INTERACTIVE-DONE` below is what the test asserts
    /// on as the captured `last_output`.
    const FAKE_CLAUDE_SCRIPT: &str = r#"
set -euo pipefail
TRANSCRIPT="$PWD/.claude/iter-test-transcript.jsonl"
mkdir -p "$(dirname "$TRANSCRIPT")"
cat > "$TRANSCRIPT" <<'EOF'
{"type":"assistant","message":{"content":[{"type":"text","text":"ITER-INTERACTIVE-DONE"}]}}
EOF
PAYLOAD=$(printf '{"session_id":"test","transcript_path":"%s","stop_hook_active":false}' "$TRANSCRIPT")
HOOK="$PWD/.claude/hooks/iter-stop-hook.sh"
printf '%s' "$PAYLOAD" | "$HOOK" > /dev/null
# Note: finalize() reads the transcript AFTER the child exits, so do NOT
# delete it here — the Rust side needs it to extract the last message.
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_captures_last_message() {
        let tmp = TempDir::new().expect("tmp");

        let (_guard, bin) = fake_binary_script(FAKE_CLAUDE_SCRIPT);
        // Drop a user-authored settings.json so we can assert it gets
        // restored intact at the end.
        let settings_path = tmp.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap())
            .await
            .expect("mkdir .claude");
        let user_settings = json!({ "user_owned": true });
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&user_settings).unwrap(),
        )
        .await
        .expect("write settings");

        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));

        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");

        assert_eq!(report.exit_status, ExitStatus::Success);
        assert_eq!(
            report.last_output.as_deref(),
            Some("ITER-INTERACTIVE-DONE"),
            "hook must capture the final assistant text message",
        );
        assert_eq!(report.turn_count, Some(1));

        // The hook bundle must have restored the user's settings.json.
        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_settings,
            "user settings.json must be restored after interactive run",
        );
        // Scratch files must be cleaned up.
        assert!(
            !tmp.path().join(".claude/hooks").exists(),
            "hooks directory must be cleaned up",
        );
        assert!(
            !tmp.path().join(".claude/iter-state.json").exists(),
            "state file must be cleaned up",
        );
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_emits_bypass_permissions_flag() {
        // Fake "claude" binary that records its argv to a file in cwd
        // then triggers the Stop hook so `finalize()` completes cleanly.
        let script = r#"
set -euo pipefail
ARGV_LOG="$PWD/.iter-argv.log"
: > "$ARGV_LOG"
for a in "$@"; do printf '%s\n' "$a" >> "$ARGV_LOG"; done
TRANSCRIPT="$PWD/.claude/iter-test-transcript.jsonl"
mkdir -p "$(dirname "$TRANSCRIPT")"
cat > "$TRANSCRIPT" <<'EOF'
{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}
EOF
PAYLOAD=$(printf '{"session_id":"t","transcript_path":"%s","stop_hook_active":false}' "$TRANSCRIPT")
HOOK="$PWD/.claude/hooks/iter-stop-hook.sh"
printf '%s' "$PAYLOAD" | "$HOOK" > /dev/null
exit 0
"#;
        let (_guard, bin) = fake_binary_script(script);
        let tmp = TempDir::new().expect("tmp");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("go");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let argv = fs::read_to_string(tmp.path().join(".iter-argv.log"))
            .await
            .expect("argv log");
        let mut lines = argv.lines();
        assert_eq!(lines.next(), Some("--permission-mode"));
        assert_eq!(lines.next(), Some("bypassPermissions"));
    }

    #[tokio::test]
    async fn interactive_mode_finalizes_even_when_child_fails() {
        // Fake claude that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("x");
        let result = agent.run(ctx(tmp.path(), &prompt)).await;

        // The child failing is surfaced as Failure(7), not an error,
        // because from iter's perspective a nonzero exit is a valid
        // report outcome. But the hook bundle MUST still be cleaned up.
        let report = result.expect("run returns Ok even on nonzero exit");
        assert_eq!(report.exit_status, ExitStatus::Failure(7));
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

    /// Fake "claude" binary that echoes its argv (one arg per line) so
    /// tests can grep for `--session-id` and the uuid that follows.
    const FAKE_ARGV_SCRIPT: &str = r#"for a in "$@"; do printf '%s\n' "$a"; done"#;

    #[tokio::test]
    async fn print_mode_without_session_id_file_emits_no_session_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(
            !out.contains("--session-id"),
            "unset session_id_file must not emit --session-id, got {out:?}",
        );
    }

    #[tokio::test]
    async fn print_mode_generates_and_writes_session_id_on_first_run() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = ClaudeAgent::new(s);

        let prompt = Prompt::from("x");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");

        // Extract the uuid that followed `--session-id` in argv.
        let mut lines = out.lines();
        let mut emitted_uuid: Option<String> = None;
        while let Some(line) = lines.next() {
            if line == "--session-id" {
                emitted_uuid = lines.next().map(str::to_string);
                break;
            }
        }
        let emitted_uuid = emitted_uuid.expect("--session-id <uuid> must appear in argv");
        // A v4 UUID parses via the uuid crate.
        let parsed =
            uuid::Uuid::parse_str(&emitted_uuid).expect("emitted session id must parse as uuid");
        assert_eq!(parsed.get_version_num(), 4, "must be a v4 uuid");

        // File must exist under the workspace (not process cwd) and hold
        // the same uuid.
        let file = tmp.path().join(".iter").join("session-id");
        let persisted = fs::read_to_string(&file).await.expect("read session id");
        assert_eq!(persisted.trim(), emitted_uuid);
    }

    #[tokio::test]
    async fn print_mode_reuses_existing_session_id_file() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        // Pre-seed a fixed uuid so we can assert exact reuse.
        let fixed = "11111111-2222-4333-8444-555555555555";
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), format!("{fixed}\n"))
            .await
            .expect("seed session id");

        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = ClaudeAgent::new(s);

        // Run twice; both runs must see the same uuid and the file must
        // be unchanged.
        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
            let out = report.last_output.expect("last_output");
            assert!(
                out.contains("--session-id"),
                "must emit --session-id, got {out:?}",
            );
            assert!(
                out.contains(fixed),
                "must reuse seeded uuid {fixed}, got {out:?}",
            );
        }
        let persisted = fs::read_to_string(tmp.path().join(".iter/session-id"))
            .await
            .expect("read");
        assert_eq!(persisted.trim(), fixed, "seeded file must not be mutated");
    }
}
