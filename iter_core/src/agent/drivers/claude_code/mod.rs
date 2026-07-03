//! [`ClaudeCodeDriver`] — Claude Code CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — assembles:
//!
//!   ```text
//!   claude --print [args...]
//!   ```
//!
//!   …with the prompt fed on stdin. `--print` tells Claude Code to emit
//!   a single response to stdout and exit — a clean, observable shape for
//!   the `AgentFinished` event payload. No tty required; works in CI and
//!   detached instances.
//!
//! * [`AgentMode::Interactive`] — assembles a live TUI invocation
//!   (`stdio: Inherit`) with a project-local Stop hook installed under
//!   `${cwd}/.claude/` by [`prepare`](crate::agent::AgentDriver::prepare)
//!   and restored by [`cleanup`](crate::agent::AgentDriver::cleanup). The
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
//!   The agent cycle guarantees `cleanup` runs on every path after a
//!   successful `prepare`, so the user's original settings are always
//!   restored — including when the session id turns out to be invalid
//!   (a leak the previous structure permitted).
//!
//! # Construction
//!
//! [`ClaudeCodeDriver`] exposes no defaults. Every field is required because
//! the value is a project-shaped decision (binary location, run mode, extra
//! flags) iter cannot honestly pick on behalf of the operator.

use std::path::{Path, PathBuf};

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;
use async_trait::async_trait;
use claude_code::{
    ClaudeCode, Error as ClaudeCodeError, ExecuteCommand, JsonOutput, PermissionMode,
};
use thiserror::Error;

mod hook;

use crate::agent::process::{
    RawOutput, apply_user_env, detect_token_limit, inject_agent_otel_resource_attrs,
    inject_trace_context_env,
};
use hook::HookBundle;

#[derive(Debug, Error)]
enum ClaudeCodeOutputError {
    /// The configured session id is not a UUID accepted by Claude Code.
    #[error("claude session id `{value}` is not a valid UUID: {source}")]
    InvalidSessionId {
        /// Invalid session id value read from the session file.
        value: String,
        /// UUID parser error.
        source: uuid::Error,
    },
    /// Context-window / token-limit detected in the output.
    #[error("claude hit the context/token limit: {0}")]
    TokenLimit(String),
    /// Claude Code output could not be decoded by the `claude_code` crate.
    #[error("claude code output could not be decoded: {0}")]
    Decode(ClaudeCodeError),
    /// A terminal `result` record with `is_error: true`.
    #[error("claude reported an error result (subtype `{subtype}`)")]
    Reported {
        /// The `subtype` of the failing record.
        subtype: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// The process exited without ever producing a terminal `result` record.
    #[error("claude produced no terminal result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

impl From<ClaudeCodeOutputError> for AgentError {
    /// Adapter projection: collapse Claude Code's CLI-shaped result/error
    /// onto iter's minimal domain error. Only token-limit detection
    /// is routing-relevant and preserved as [`AgentError::TokenLimit`]; the
    /// rest become generic failures.
    fn from(err: ClaudeCodeOutputError) -> Self {
        match err {
            ClaudeCodeOutputError::InvalidSessionId { value, source } => {
                Self::Launch(format!("invalid claude session id `{value}`: {source}"))
            }
            ClaudeCodeOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            ClaudeCodeOutputError::Decode(source) => Self::Failed {
                code: None,
                message: format!("claude code output could not be decoded: {source}"),
            },
            ClaudeCodeOutputError::Reported { subtype, exit_code } => Self::Failed {
                code: exit_code,
                message: format!("claude reported error result `{subtype}`"),
            },
            ClaudeCodeOutputError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "claude produced no terminal result".to_owned(),
            },
        }
    }
}

/// Claude Code driver configuration.
#[derive(Debug, Clone)]
pub struct ClaudeCodeDriver {
    /// Binary name or path. Required (no implicit `"claude"` fallback).
    pub command: String,
    /// How iter drives the Claude Code process. Required (no implicit fallback).
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags. Useful for
    /// overriding assumptions like `--model` or `--output-format`.
    pub args: Vec<String>,
    /// Optional path (relative to the workspace cwd, unless absolute) of a
    /// file that stores a stable Claude Code session id across iterations.
    ///
    /// When set, the agent cycle resolves the file (generating and
    /// persisting a fresh v4 UUID on first use) and every invocation passes
    /// `--session-id <uuid>` — creating the session on the first run and
    /// resuming it on every later one. This is the narrowest exploration
    /// mode because accumulated agent context keeps later turns close to
    /// earlier ones. Lifecycle (deleting the file to end an exploration
    /// run) is left to the caller; iter has no notion of "end of
    /// exploration".
    pub session_id_file: Option<PathBuf>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
    /// Per-exploration hook isolation key: distinguishes one Runner's
    /// stop-hook installation from another's when both explore the same
    /// workspace path. `"default"` for standalone `iter run`.
    pub hook_isolation_key: String,
}

fn home_subpath(leaf: &str) -> Option<PathBuf> {
    // Routes through the single core base-dir helper, which treats an empty
    // `$HOME` as unset (`None`) — intentional; do not revert to a raw
    // `var_os("HOME")` that would yield a bogus `"".join(leaf)`.
    crate::home::home_dir().map(|h| h.join(leaf))
}

impl ClaudeCodeDriver {
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

    /// Parse the resolved session token as the UUID Claude Code requires.
    fn parse_session(session: Option<&str>) -> Result<Option<uuid::Uuid>, AgentError> {
        session
            .map(uuid::Uuid::parse_str)
            .transpose()
            .map_err(|source| {
                ClaudeCodeOutputError::InvalidSessionId {
                    value: session.unwrap_or_default().to_owned(),
                    source,
                }
                .into()
            })
    }
}

#[async_trait]
impl AgentDriver for ClaudeCodeDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        let session_id = Self::parse_session(session)?;
        let execute = ExecuteCommand {
            permission_mode: Some(PermissionMode::BypassPermissions),
            session_id,
            ..ExecuteCommand::default()
        };
        match self.mode {
            AgentMode::Headless => {
                let mut process = ClaudeCode::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&execute.json());
                process.args(&self.args);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "claude");
                if inject_trace_context_env(&mut process) {
                    process.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1");
                }
                Ok(AgentCommand {
                    process,
                    stdin: Some(prompt.as_str().to_owned()),
                    io: StdioMode::Piped,
                })
            }
            AgentMode::Interactive => {
                let mut process = ClaudeCode::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&execute);
                process.arg(prompt.as_str());
                process.args(&self.args);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "claude");
                Ok(AgentCommand {
                    process,
                    stdin: None,
                    io: StdioMode::Inherit,
                })
            }
        }
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        let raw = RawOutput::from(output);
        match self.mode {
            // Interactive mode has no machine-readable output: the only
            // signal is the child's exit. A clean exit is a run; anything
            // else is a failure.
            AgentMode::Interactive => match raw.exit.into_failure() {
                None => Ok(AgentRun::empty()),
                Some(err) => Err(err),
            },
            AgentMode::Headless => {
                let exit_code = raw.exit.exit_code();
                let stdout_text = raw.stdout_str().into_owned();
                let stderr_text = raw.stderr_str().into_owned();
                // `JsonOutput::try_from` takes the output by value; one
                // clone of the byte streams is the cost of the external
                // crate's signature.
                let result = match JsonOutput::try_from(output.clone()) {
                    Ok(result) => result,
                    Err(ClaudeCodeError::Cli {
                        exit_code,
                        stdout,
                        stderr,
                    }) => {
                        if let Some(err) = raw.exit.into_failure()
                            && matches!(err, AgentError::TerminatedBySignal(_))
                        {
                            return Err(err);
                        }
                        if let Some(detail) = detect_token_limit(&stdout) {
                            return Err(ClaudeCodeOutputError::TokenLimit(detail).into());
                        }
                        if let Some(detail) = detect_token_limit(&stderr) {
                            return Err(ClaudeCodeOutputError::TokenLimit(detail).into());
                        }
                        return Err(ClaudeCodeOutputError::NoResult { exit_code }.into());
                    }
                    Err(err) => return Err(ClaudeCodeOutputError::Decode(err).into()),
                };
                if result.is_error {
                    if let Some(detail) = result
                        .result
                        .as_deref()
                        .and_then(detect_token_limit)
                        .or_else(|| detect_token_limit(&stdout_text))
                        .or_else(|| detect_token_limit(&stderr_text))
                    {
                        return Err(ClaudeCodeOutputError::TokenLimit(detail).into());
                    }
                    return Err(ClaudeCodeOutputError::Reported {
                        subtype: result.subtype.as_str().to_owned(),
                        exit_code,
                    }
                    .into());
                }
                Ok(AgentRun {
                    session_id: result.session_id,
                })
            }
        }
    }

    async fn prepare(&self, path: &Path) -> Result<(), AgentError> {
        if matches!(self.mode, AgentMode::Interactive) {
            // The bundle handle is a pure path derivation; cleanup
            // reattaches to it, so the driver holds no state between the
            // two calls.
            drop(HookBundle::install(path, &self.hook_isolation_key).await?);
        }
        Ok(())
    }

    async fn cleanup(&self, path: &Path) -> Result<(), AgentError> {
        if matches!(self.mode, AgentMode::Interactive) {
            HookBundle::reattach(path).finalize().await?;
        }
        Ok(())
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    ///
    /// The returned handle exposes both the resolved path and its canonical
    /// target so the sandbox layer can grant read access to a symlink shim
    /// (volta, nvm, asdf, homebrew cask).
    fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }

    fn session_file(&self) -> Option<&Path> {
        self.session_id_file.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RawExit;
    use crate::agent::testutil::{drive, drive_capturing, fake_binary_script};
    use serde_json::json;
    use std::ffi::OsStr;
    use tempfile::TempDir;
    use tokio::fs;

    fn driver(command: impl Into<String>, mode: AgentMode) -> ClaudeCodeDriver {
        ClaudeCodeDriver {
            command: command.into(),
            mode,
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
            hook_isolation_key: "default".to_owned(),
        }
    }

    fn argv(command: &AgentCommand) -> Vec<String> {
        command
            .process
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn synth_output(exit: RawExit, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: exit.into_exit_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x"}"#;

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn headless_command_emits_print_json_and_bypass_permissions() {
        let d = driver("claude", AgentMode::Headless);
        let prompt = Prompt::from("hello");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--print".to_owned()), "got {args:?}");
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        assert!(
            args.contains(&"--permission-mode".to_owned()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"bypassPermissions".to_owned()),
            "got {args:?}"
        );
        assert_eq!(command.stdin.as_deref(), Some("hello"));
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_without_session_emits_no_session_flag() {
        let d = driver("claude", AgentMode::Headless);
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert!(
            !argv(&command).contains(&"--session-id".to_owned()),
            "no session token must mean no --session-id",
        );
    }

    #[test]
    fn headless_command_passes_valid_session_uuid() {
        let d = driver("claude", AgentMode::Headless);
        let prompt = Prompt::from("x");
        let fixed = "11111111-2222-4333-8444-555555555555";
        let command = d
            .command(Path::new("."), &prompt, Some(fixed))
            .expect("command");
        let args = argv(&command);
        let pos = args
            .iter()
            .position(|a| a == "--session-id")
            .expect("--session-id present");
        assert_eq!(args[pos + 1], fixed);
    }

    #[test]
    fn invalid_session_token_is_a_launch_error() {
        let d = driver("claude", AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = d
            .command(Path::new("."), &prompt, Some("not-a-uuid"))
            .expect_err("invalid uuid must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn extra_args_are_appended() {
        let mut d = driver("claude", AgentMode::Headless);
        d.args = vec!["--model".into(), "opus".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"opus".to_owned()), "got {args:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("claude", AgentMode::Headless);
        d.env = vec![("ITER_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == OsStr::new("ITER_TEST_ENV_VAR") && v == Some(OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_embeds_prompt_and_inherits_stdio() {
        let d = driver("claude", AgentMode::Interactive);
        let prompt = Prompt::from("go");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(
            args.contains(&"go".to_owned()),
            "prompt as argv, got {args:?}"
        );
        assert!(!args.contains(&"--print".to_owned()));
        // The interactive branch builds through a different `to_process`
        // overload than headless; pin that it still carries the permission
        // mode.
        assert!(
            args.contains(&"--permission-mode".to_owned()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"bypassPermissions".to_owned()),
            "got {args:?}"
        );
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    // ----- interpret(): inbound translation --------------------------------

    #[test]
    fn interpret_success_result_extracts_session_id() {
        let d = driver("claude", AgentMode::Headless);
        let run = d
            .interpret(&synth_output(RawExit::Code(0), RESULT_OK, ""))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_is_error_result_is_reported_failure() {
        let d = driver("claude", AgentMode::Headless);
        let body = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom","session_id":"s"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(0), body, ""))
            .expect_err("is_error must fail");
        assert!(
            matches!(err, AgentError::Failed { ref message, .. } if message.contains("error_during_execution")),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_in_stderr_classifies() {
        let d = driver("claude", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                "",
                "error: context window exceeded",
            ))
            .expect_err("token limit must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_no_result_is_failure() {
        let d = driver("claude", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(7), "plain text", ""))
            .expect_err("no terminal record must fail");
        assert!(matches!(err, AgentError::Failed { .. }), "got {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn interpret_signal_termination_survives() {
        let d = driver("claude", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), "", ""))
            .expect_err("signal must fail");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}"
        );
    }

    #[test]
    fn interpret_interactive_judges_by_exit_only() {
        let d = driver("claude", AgentMode::Interactive);
        assert!(d.interpret(&synth_output(RawExit::Code(0), "", "")).is_ok());
        let err = d
            .interpret(&synth_output(RawExit::Code(7), "", ""))
            .expect_err("non-zero exit");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `claude` print binary: echoes each argv arg and its stdin to
    /// *stderr* (so the capture sink can observe them), then prints a valid
    /// terminal `result` JSON object to stdout.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x"}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_flag_and_stdin() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-claude");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--print"), "got {echoed:?}");
        assert!(echoed.contains("hello-claude"), "got {echoed:?}");
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

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
        let mut d = driver(bin.to_string_lossy(), AgentMode::Headless);
        d.session_id_file = Some(PathBuf::from(".iter/session-id"));

        let prompt = Prompt::from("x");
        let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
        result.expect("run ok");

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

        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
            let mut d = driver(bin.to_string_lossy(), AgentMode::Headless);
            d.session_id_file = Some(PathBuf::from(".iter/session-id"));
            let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
            result.expect("run ok");
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

    // -----------------------------------------------------------------
    // interactive mode: hook lifecycle through prepare/cleanup.
    // -----------------------------------------------------------------

    /// Fake `claude` binary for interactive mode.
    ///
    /// Invokes the installed Stop hook with a dummy payload on stdin.
    /// The hook drains stdin and SIGKILLs `$PPID` (this fake process),
    /// causing it to exit. This drives the real hook path end-to-end
    /// without needing a tty or the actual `claude` binary.
    const FAKE_CLAUDE_SCRIPT: &str = r#"
set -eu
HOOK="$PWD/.claude/hooks/iter-stop-hook.sh"
# Invoke the hook — it will drain stdin and SIGKILL us ($PPID from
# its perspective). The hook runs in a subshell so its kill targets us.
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

        let d = driver(bin.to_string_lossy(), AgentMode::Interactive);

        let prompt = Prompt::from("go");
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that cleanup restored settings.
        let _ignored = drive(d, tmp.path(), &prompt).await;

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
    async fn interactive_mode_cleans_up_even_when_child_fails() {
        // Fake claude that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let d = driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let result = drive(d, tmp.path(), &prompt).await;

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

    /// Regression net for the leak the previous structure permitted: on an
    /// interactive run whose session file holds an invalid UUID, the hook
    /// bundle was installed but never finalized (the session validation
    /// error skipped the finalize). In the cycle, prepare's success
    /// guarantees cleanup, so the settings are restored.
    #[tokio::test]
    async fn interactive_invalid_session_id_still_restores_settings() {
        let tmp = TempDir::new().expect("tmp");
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
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir .iter");
        fs::write(tmp.path().join(".iter/session-id"), "not-a-uuid\n")
            .await
            .expect("seed invalid session id");

        let (_guard, bin) = fake_binary_script("exit 0");
        let mut d = driver(bin.to_string_lossy(), AgentMode::Interactive);
        d.session_id_file = Some(PathBuf::from(".iter/session-id"));

        let prompt = Prompt::from("x");
        let err = drive(d, tmp.path(), &prompt)
            .await
            .expect_err("invalid session id must fail the run");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");

        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_settings,
            "settings must be restored even when the session id is invalid",
        );
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must not leak on the invalid-session path",
        );
    }
}
