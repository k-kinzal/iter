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
//!   hook's sole purpose is to terminate the TUI session after the agent
//!   finishes its task — it runs any pre-existing user Stop hooks, then
//!   sends SIGKILL to the Claude Code process. The hook is a direct
//!   descendant of
//!   [`agent-loop/claude-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) already
//!   handles signal-level iteration, so the hook only needs to terminate
//!   the TUI session.
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

use crate::{Agent, AgentRun, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod command;
mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize,
    inject_agent_otel_resource_attrs, inject_trace_context_env, spawn_capture,
};
use crate::agent::session::SessionIdFile;
use command::{ClaudeCodeCommand, ClaudeCodeError};
use hook::HookBundle;

impl From<ClaudeCodeError> for AgentError {
    /// Adapter projection: collapse Claude Code's CLI-shaped error hierarchy
    /// onto iter's minimal domain error. Only [`ClaudeCodeError::TokenLimit`]
    /// is router-relevant and preserved as [`AgentError::TokenLimit`]; the
    /// rest become the generic failure / signal variants.
    fn from(err: ClaudeCodeError) -> Self {
        match err {
            ClaudeCodeError::TokenLimit(detail) => Self::TokenLimit(detail),
            ClaudeCodeError::Signal(sig) => Self::TerminatedBySignal(sig),
            ClaudeCodeError::Reported { subtype, exit_code } => Self::Failed {
                code: exit_code,
                message: format!("claude reported error result `{subtype}`"),
            },
            ClaudeCodeError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "claude produced no terminal result".to_owned(),
            },
        }
    }
}

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
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
            env,
        } = settings;
        Self {
            command,
            mode,
            args,
            session_id_file,
            env,
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
    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            service_name,
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
                let mut command = ClaudeCodeCommand {
                    program: &self.command,
                    args: &self.args,
                    session_id: session_id.as_deref(),
                }
                .build(workspace_path);
                apply_user_env(&mut command, &self.env);
                inject_agent_otel_resource_attrs(
                    &mut command,
                    signal_id,
                    signal_kind,
                    workspace_path,
                    "claude",
                );
                if inject_trace_context_env(&mut command) {
                    command.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1");
                }
                let output = spawn_capture(
                    command,
                    PromptDelivery::Stdin(prompt.as_str()),
                    cancel,
                    stdio_sink,
                )
                .await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<ClaudeCodeError>` above.
                let result = command::interpret(&output)?;
                Ok(AgentRun {
                    session_id: result.session_id,
                })
            }
            AgentMode::Interactive => {
                self.run_interactive(
                    workspace_path,
                    prompt,
                    session_id.as_deref(),
                    cancel,
                    signal_id,
                    signal_kind,
                    &service_name,
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
    #[allow(clippy::too_many_arguments)]
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        session_id: Option<&str>,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
        service_name: &str,
    ) -> Result<AgentRun, AgentError> {
        let bundle = HookBundle::install(path, service_name).await?;

        let mut command = self.build_interactive_command(path, prompt, session_id);
        apply_user_env(&mut command, &self.env);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "claude");
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        // Interactive mode has no machine-readable output: the only signal is
        // the child's exit. A clean exit is a run; anything else is a failure.
        let exit = drive_interactive_with_finalize(command, cancel, bundle.finalize()).await?;
        if let Some(err) = exit.into_failure() {
            return Err(err);
        }
        Ok(AgentRun::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{ctx, ctx_capturing, fake_binary_script};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>, mode: AgentMode) -> ClaudeSettings {
        ClaudeSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    /// Fake `claude` print binary: echoes each argv arg and its stdin to
    /// *stderr* (so a [`CaptureSink`] can observe them), then prints a valid
    /// terminal `result` JSON object to stdout so the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x"}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_flag_and_stdin() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-claude");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--print"), "got {echoed:?}");
        assert!(echoed.contains("hello-claude"), "got {echoed:?}");
    }

    #[tokio::test]
    async fn print_mode_emits_output_format_json_and_bypass_permissions() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
        assert!(args.contains(&"--permission-mode"), "got {args:?}");
        assert!(args.contains(&"bypassPermissions"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$ITER_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"type\":\"result\",\"is_error\":false,\"session_id\":\"s\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("ITER_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = ClaudeAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn print_mode_extra_args_are_forwarded_after_print_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "opus".into()];
        let agent = ClaudeAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--print"), "got {args:?}");
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"opus"), "got {args:?}");
    }

    /// Fake `claude` binary for interactive mode.
    ///
    /// Invokes the installed Stop hook with a dummy payload on stdin.
    /// The hook drains stdin and SIGKILLs `$PPID` (this fake process),
    /// causing it to exit. This drives the real
    /// [`HookBundle::finalize`] path end-to-end without needing a tty or
    /// the actual `claude` binary.
    const FAKE_CLAUDE_SCRIPT: &str = r#"
set -euo pipefail
HOOK="$PWD/.claude/hooks/iter-stop-hook.sh"
# Invoke the hook — it will drain stdin and SIGKILL us ($PPID from
# its perspective). We trap KILL so the test can observe a clean
# exit path. The hook runs in a subshell so its kill targets us.
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_settings() {
        let tmp = TempDir::new().expect("tmp");

        let (_guard, bin) = fake_binary_script(FAKE_CLAUDE_SCRIPT);
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
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that the bundle was finalized.
        let _ignored = agent.run(ctx(tmp.path(), &prompt)).await;

        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_settings,
            "user settings.json must be restored after interactive run",
        );
        assert!(
            !tmp.path().join(".claude/hooks").exists(),
            "hooks directory must be cleaned up",
        );
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_emits_bypass_permissions_flag() {
        let script = r#"
set -euo pipefail
ARGV_LOG="$PWD/.iter-argv.log"
: > "$ARGV_LOG"
for a in "$@"; do printf '%s\n' "$a" >> "$ARGV_LOG"; done
exit 0
"#;
        let (_guard, bin) = fake_binary_script(script);
        let tmp = TempDir::new().expect("tmp");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("go");
        let _report = agent.run(ctx(tmp.path(), &prompt)).await;
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

        // A non-zero exit is now an `Err(Failed { code: Some(7) })` — the
        // agent ran no clean turn. The hook bundle MUST still be cleaned up.
        let err = result.expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn print_mode_without_session_id_file_emits_no_session_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let tmp = TempDir::new().expect("tmp");
        let agent = ClaudeAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(
            !sink.stderr().await.lines().any(|l| l == "--session-id"),
            "unset session_id_file must not emit --session-id",
        );
    }

    /// Extract the uuid emitted after `--session-id` in the captured argv.
    fn session_id_from_argv(echoed: &str) -> Option<String> {
        let mut lines = echoed.lines();
        while let Some(line) = lines.next() {
            if line == "--session-id" {
                return lines.next().map(str::to_string);
            }
        }
        None
    }

    #[tokio::test]
    async fn print_mode_generates_and_writes_session_id_on_first_run() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let tmp = TempDir::new().expect("tmp");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = ClaudeAgent::new(s);

        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");

        let emitted_uuid =
            session_id_from_argv(&sink.stderr().await).expect("--session-id <uuid> in argv");
        let parsed =
            uuid::Uuid::parse_str(&emitted_uuid).expect("emitted session id must parse as uuid");
        assert_eq!(parsed.get_version_num(), 4, "must be a v4 uuid");

        let file = tmp.path().join(".iter").join("session-id");
        let persisted = fs::read_to_string(&file).await.expect("read session id");
        assert_eq!(persisted.trim(), emitted_uuid);
    }

    #[tokio::test]
    async fn print_mode_reuses_existing_session_id_file() {
        let tmp = TempDir::new().expect("tmp");
        let fixed = "11111111-2222-4333-8444-555555555555";
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), format!("{fixed}\n"))
            .await
            .expect("seed session id");

        let mut s = settings("placeholder", AgentMode::Print);
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));

        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
            s.command = bin.to_string_lossy().into_owned();
            let agent = ClaudeAgent::new(s.clone());
            let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
            agent.run(ctx).await.expect("run ok");
            assert_eq!(
                session_id_from_argv(&sink.stderr().await).as_deref(),
                Some(fixed),
                "must reuse seeded uuid",
            );
        }
        let persisted = fs::read_to_string(tmp.path().join(".iter/session-id"))
            .await
            .expect("read");
        assert_eq!(persisted.trim(), fixed, "seeded file must not be mutated");
    }
}
