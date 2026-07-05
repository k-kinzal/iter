//! [`GeminiDriver`] — Google Gemini CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles:
//!
//!   ```text
//!   gemini --output-format json --prompt <prompt> [extra-args...]
//!   ```
//!
//!   The prompt is delivered as the value of `--prompt` and
//!   `--output-format json` requests the machine-readable terminal record,
//!   both rendered by the `gemini_cli` crate's `RunCommand`. The child's
//!   stdin is closed immediately and stdout is captured for the driver to
//!   interpret into an [`AgentRun`] or [`AgentError`].
//!
//! * [`AgentMode::Interactive`] — launches `gemini` as a live TUI
//!   (`stdio: Inherit`) with a project-local `AfterAgent` hook installed
//!   under `${cwd}/.gemini/` by
//!   [`prepare`](crate::agent::AgentDriver::prepare) and restored by
//!   [`cleanup`](crate::agent::AgentDriver::cleanup). The hook's sole
//!   purpose is to terminate the TUI session after the agent finishes its
//!   task — it runs any pre-existing user hooks, then sends SIGKILL to the
//!   Gemini CLI process.
//!
//!   The hook is a direct descendant of
//!   [`agent-loop/gemini-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to terminate the
//!   TUI session.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.gemini/`. iter never writes to `~/.gemini/` because
//!   doing so would silently affect every other Gemini CLI session on
//!   the machine. See the `hook` submodule for the filesystem layout.
//!
//!   The agent cycle guarantees `cleanup` runs on every path after a
//!   successful `prepare`, so the user's original `settings.json` is always
//!   restored.
//!
//! # Construction
//!
//! [`GeminiDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;
use async_trait::async_trait;
use tokio::process::Command;

mod hook;

use gemini_cli::{Gemini, GeminiOutput, RunCommand};
use thiserror::Error;

use crate::agent::process::{
    RawExit, RawOutput, apply_user_env, detect_token_limit, inject_agent_otel_resource_attrs,
    inject_trace_context_env,
};
use hook::HookBundle;

/// Fatal startup exit codes (auth / input / sandbox / config / turn-limit);
/// a process that exits with one never ran a turn → [`AgentError::Launch`].
const STARTUP_EXIT_CODES: &[i32] = &[41, 42, 44, 52, 53];

/// CLI-shaped error hierarchy for the Gemini CLI's headless output.
///
/// This is the neutral classification the driver derives from a completed
/// `gemini --output-format json` run before projecting it onto iter's domain
/// [`AgentError`] via the `From` impl below. The `gemini_cli` crate reports
/// the raw terminal record faithfully; deciding what that record *means* is
/// this driver's responsibility — the anti-corruption boundary between the
/// CLI's vocabulary and iter's.
#[derive(Debug, Error)]
enum GeminiOutputError {
    /// Context-window / token-limit detected in the output or `error.message`.
    #[error("gemini hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A fatal startup exit code (auth / input / sandbox / config /
    /// turn-limit). The agent never ran a turn.
    #[error("gemini failed to start (exit code {exit_code})")]
    Startup {
        /// The fatal startup exit code (one of [`STARTUP_EXIT_CODES`]).
        exit_code: i32,
        /// Diagnostic message, when one was parsed from the JSON `error`.
        message: Option<String>,
    },
    /// A terminal record carrying an `error` field (in-band failure).
    #[error("gemini reported an error result")]
    Reported {
        /// `error.type`, when present.
        error_type: Option<String>,
        /// `error.message`, when present.
        message: Option<String>,
        /// `error.code`, or the process exit code when no JSON code was given.
        code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("gemini was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a JSON result object.
    #[error("gemini produced no result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Is this exit code one of the fatal startup codes?
fn is_startup_code(exit_code: Option<i32>) -> Option<i32> {
    exit_code.filter(|c| STARTUP_EXIT_CODES.contains(c))
}

/// Does this `error.type` look like a context/token-limit class?
fn is_context_error_type(error_type: Option<&str>) -> bool {
    error_type.is_some_and(|t| {
        let lower = t.to_ascii_lowercase();
        lower.contains("context") || lower.contains("token")
    })
}

impl From<GeminiOutputError> for AgentError {
    /// Adapter projection: collapse the Gemini CLI's CLI-shaped error
    /// hierarchy onto iter's minimal domain error.
    ///
    /// * Context/token-limit → [`AgentError::TokenLimit`] (router-relevant).
    /// * Fatal startup exit codes (auth / input / sandbox / config /
    ///   turn-limit) → [`AgentError::Launch`] — the agent never ran a turn.
    /// * Signal termination → [`AgentError::TerminatedBySignal`].
    /// * Everything else → [`AgentError::Failed`].
    fn from(err: GeminiOutputError) -> Self {
        match err {
            GeminiOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            GeminiOutputError::Startup { exit_code, message } => Self::Launch(match message {
                Some(msg) => format!("gemini startup failure (exit code {exit_code}): {msg}"),
                None => format!("gemini startup failure (exit code {exit_code})"),
            }),
            GeminiOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
            GeminiOutputError::Reported {
                error_type,
                message,
                code,
            } => Self::Failed {
                code,
                message: match (error_type, message) {
                    (Some(t), Some(m)) => format!("gemini reported error `{t}`: {m}"),
                    (Some(t), None) => format!("gemini reported error `{t}`"),
                    (None, Some(m)) => format!("gemini reported error: {m}"),
                    (None, None) => "gemini reported an error result".to_owned(),
                },
            },
            GeminiOutputError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "gemini produced no result".to_owned(),
            },
        }
    }
}

/// Gemini CLI driver configuration.
#[derive(Debug, Clone)]
pub struct GeminiDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags (the
    /// `--output-format json --prompt <prompt>` pair in print mode, or the
    /// prompt positional in interactive mode).
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
    /// Per-exploration hook isolation key: distinguishes one Runner's
    /// stop-hook installation from another's when both explore the same
    /// workspace path. `"default"` for standalone `iter run`.
    pub hook_isolation_key: String,
}

impl GeminiDriver {
    /// Build the interactive-mode command. Passes the prompt as the
    /// first positional argument so `gemini` seeds its initial user
    /// turn with it before dropping into the TUI; extras come after so
    /// users can still inject their own flags.
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

#[async_trait]
impl AgentDriver for GeminiDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        match self.mode {
            AgentMode::Headless => {
                // The `gemini_cli` crate renders the managed flags
                // (`--output-format json --prompt <prompt>`); the caller's
                // extra args are appended after so they can still override.
                let mut process = Gemini::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&RunCommand::prompt(prompt.as_str()).json());
                process.args(&self.args);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "gemini");
                inject_trace_context_env(&mut process);
                Ok(AgentCommand {
                    // The prompt is already on the argv (`--prompt`), so no
                    // stdin payload is sent; the cycle closes stdin immediately.
                    process,
                    stdin: None,
                    io: StdioMode::Piped,
                })
            }
            AgentMode::Interactive => {
                let mut process = self.build_interactive_command(path, prompt);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "gemini");
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
                // Adapter: parse the neutral CLI record with the `gemini_cli`
                // crate, then classify it into iter's domain. Every `.into()`
                // runs the `From<GeminiOutputError>` above.
                let stdout = raw.stdout_str();
                let stderr = raw.stderr_str();
                let exit_code = match raw.exit {
                    RawExit::Code(c) => Some(c),
                    RawExit::Signal(_) | RawExit::Unknown => None,
                };

                let Some(record) = GeminiOutput::parse(&stdout) else {
                    // No JSON object → the agent never produced a result.
                    if let RawExit::Signal(sig) = raw.exit {
                        return Err(GeminiOutputError::Signal(sig).into());
                    }
                    if let Some(detail) =
                        detect_token_limit(&stdout).or_else(|| detect_token_limit(&stderr))
                    {
                        return Err(GeminiOutputError::TokenLimit(detail).into());
                    }
                    if let Some(code) = is_startup_code(exit_code) {
                        return Err(GeminiOutputError::Startup {
                            exit_code: code,
                            message: None,
                        }
                        .into());
                    }
                    return Err(GeminiOutputError::NoResult { exit_code }.into());
                };

                if let Some(err) = record.error() {
                    let message = err.message.clone();
                    // Refine into token-limit when the type or message says so.
                    if is_context_error_type(err.error_type.as_deref()) {
                        let detail = message
                            .as_deref()
                            .and_then(detect_token_limit)
                            .or_else(|| message.clone())
                            .unwrap_or_else(|| "context/token limit".to_owned());
                        return Err(GeminiOutputError::TokenLimit(detail).into());
                    }
                    if let Some(detail) = message
                        .as_deref()
                        .and_then(detect_token_limit)
                        .or_else(|| detect_token_limit(&stdout))
                    {
                        return Err(GeminiOutputError::TokenLimit(detail).into());
                    }
                    if let Some(code) = is_startup_code(exit_code) {
                        return Err(GeminiOutputError::Startup {
                            exit_code: code,
                            message,
                        }
                        .into());
                    }
                    return Err(GeminiOutputError::Reported {
                        error_type: err.error_type,
                        message,
                        code: err.code.or(exit_code),
                    }
                    .into());
                }

                // No `error` field, but a startup exit code still overrides a
                // stray object.
                if let Some(code) = is_startup_code(exit_code) {
                    return Err(GeminiOutputError::Startup {
                        exit_code: code,
                        message: None,
                    }
                    .into());
                }

                Ok(AgentRun {
                    session_id: record.session_id(),
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
        AgentKind::Gemini
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
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

    fn gemini_driver(command: impl Into<String>, mode: AgentMode) -> GeminiDriver {
        GeminiDriver {
            command: command.into(),
            mode,
            args: Vec::new(),
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

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn headless_command_emits_prompt_and_json_format() {
        let d = gemini_driver("gemini", AgentMode::Headless);
        let prompt = Prompt::from("hello-gemini");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--prompt".to_owned()), "got {args:?}");
        assert!(args.contains(&"hello-gemini".to_owned()), "got {args:?}");
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        let flag_pos = args.iter().position(|a| a == "--prompt").expect("--prompt");
        let prompt_pos = args
            .iter()
            .position(|a| a == "hello-gemini")
            .expect("prompt");
        assert!(flag_pos < prompt_pos, "got {args:?}");
        assert_eq!(command.stdin, None, "gemini delivers the prompt as argv");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_forwards_extra_args_after_managed_flags() {
        let mut d = gemini_driver("gemini", AgentMode::Headless);
        d.args = vec!["--model".into(), "gemini-pro".into()];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"gemini-pro".to_owned()), "got {args:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = gemini_driver("gemini", AgentMode::Headless);
        d.env = vec![("GEMINI_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == OsStr::new("GEMINI_TEST_ENV_VAR") && v == Some(OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_puts_prompt_first_and_inherits_stdio() {
        let d = gemini_driver("gemini", AgentMode::Interactive);
        let prompt = Prompt::from("go");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert_eq!(args.first().map(String::as_str), Some("go"), "got {args:?}");
        assert!(!args.contains(&"-p".to_owned()));
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    // ----- interpret(): inbound translation --------------------------------

    #[test]
    fn interpret_session_id_when_present() {
        let d = gemini_driver("gemini", AgentMode::Headless);
        let body = r#"{"response":"ok","session_id":"conv-9"}"#;
        let run = d
            .interpret(&synth_output(RawExit::Code(0), body, ""))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("conv-9"));
    }

    #[test]
    fn interpret_error_field_maps_to_failed() {
        let d = gemini_driver("gemini", AgentMode::Headless);
        let body = r#"{"error":{"type":"ApiError","message":"boom","code":7}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body, ""))
            .expect_err("err");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn interpret_startup_exit_code_maps_to_launch() {
        let d = gemini_driver("gemini", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(41), "", ""))
            .expect_err("err");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn interpret_context_error_maps_to_token_limit() {
        let d = gemini_driver("gemini", AgentMode::Headless);
        let body = r#"{"error":{"type":"ContextLengthExceeded","message":"too big"}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body, ""))
            .expect_err("err");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_interactive_judges_by_exit_only() {
        let d = gemini_driver("gemini", AgentMode::Interactive);
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

    /// Fake `gemini` print binary: echoes each argv arg to *stderr* (so a
    /// capture sink can observe them), then prints a valid terminal JSON
    /// object to stdout so the driver parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"response":"ok","stats":{"tokens":{"input":1,"output":2,"total":3}}}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_argv() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = gemini_driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-gemini");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--prompt"), "got {args:?}");
        assert!(args.contains(&"hello-gemini"), "got {args:?}");
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
    }

    /// Fake `gemini` binary for interactive mode. Invokes the installed
    /// `AfterAgent` hook. The hook drains stdin and SIGKILLs `$PPID`.
    const FAKE_GEMINI_SCRIPT: &str = r#"
set -euo pipefail
HOOK="$PWD/.gemini/hooks/gemini-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_settings() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_GEMINI_SCRIPT);

        let settings_path = tmp.path().join(".gemini/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap())
            .await
            .expect("mkdir .gemini");
        let user_settings = json!({ "user_owned": true });
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&user_settings).unwrap(),
        )
        .await
        .expect("write settings");

        let d = gemini_driver(bin.to_string_lossy(), AgentMode::Interactive);

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
            !tmp.path().join(".gemini/hooks").exists(),
            "hooks directory must be cleaned up",
        );
        assert!(
            !tmp.path().join(".gemini/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_cleans_up_even_when_child_fails() {
        // Fake gemini that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let d = gemini_driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let result = drive(d, tmp.path(), &prompt).await;

        // A non-zero exit is an `Err(Failed { code: Some(7) })` — the agent
        // ran no clean turn. The hook bundle MUST still be cleaned up.
        let err = result.expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
        assert!(
            !tmp.path().join(".gemini/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }
}
