//! [`CodexDriver`] — `OpenAI` Codex CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles:
//!
//!   ```text
//!   codex exec --json [extra-args...] <prompt>
//!   ```
//!
//!   The prompt is passed as the final positional argument and the
//!   child's stdin is closed immediately. `exec` is Codex's one-shot
//!   non-interactive mode; `--json` requests the machine-readable JSONL
//!   event stream the Command layer parses for the terminal turn status
//!   and session id. See the `command` submodule for the output contract.
//!
//! * [`AgentMode::Interactive`] — launches `codex` as a live TUI
//!   (`stdio: Inherit`) with a project-local Stop hook installed under
//!   `${cwd}/.codex/` by [`prepare`](crate::agent::AgentDriver::prepare)
//!   and restored by [`cleanup`](crate::agent::AgentDriver::cleanup).
//!   Codex ships Claude-Code-style Stop hooks behind a CLI feature flag,
//!   so interactive mode invokes the binary as:
//!
//!   ```text
//!   codex --config "features.codex_hooks=true" [extra-args...] <prompt>
//!   ```
//!
//!   The hook's sole purpose is to terminate the TUI session after the
//!   agent finishes its task — it runs any pre-existing user Stop hooks,
//!   then sends SIGKILL to the Codex process. The hook is a direct
//!   descendant of
//!   [`agent-loop/codex-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to terminate the
//!   TUI session.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.codex/`. iter never writes to `~/.codex/` because
//!   doing so would silently affect every other Codex session on the
//!   machine. See the `hook` submodule for the filesystem layout.
//!
//!   The agent cycle guarantees `cleanup` runs on every path after a
//!   successful `prepare`, so the user's original `hooks.json` is always
//!   restored.
//!
//! # Assumptions to verify later
//!
//! - The subcommand for print mode is `exec`. Some Codex builds use
//!   `run` or a bare prompt.
//! - `codex` accepts `--config "features.codex_hooks=true"` to enable the Stop
//!   hook protocol in interactive mode.
//! - The prompt is a positional argument, not a `--prompt=...` flag.
//!
//! Override [`args`](CodexDriver::args) to swap the subcommand or inject
//! flags without recompiling.
//!
//! # Construction
//!
//! [`CodexDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;
use async_trait::async_trait;
use codex_cli::{Codex, CommonConfig, ConfigOverride, ExecCommand, RunCommand};
use thiserror::Error;

mod hook;

use crate::agent::process::{
    RawExit, RawOutput, apply_user_env, detect_token_limit, inject_agent_otel_resource_attrs,
    inject_trace_context_env,
};
use hook::HookBundle;

/// Config override that enables Codex's Stop hook protocol in interactive mode.
const CODEX_HOOKS_FEATURE_CONFIG_KEY: &str = "features.codex_hooks";
const CODEX_HOOKS_FEATURE_CONFIG_VALUE: &str = "true";

/// Codex's usage-limit message — treated as a token/usage-limit class even
/// though it is not one of the generic context-window patterns.
const USAGE_LIMIT_MESSAGE: &str = "You've hit your usage limit.";

/// Clap argument-parse rejection exit code.
const EXIT_BAD_ARGS: i32 = 2;

/// CLI-shaped classification of a Codex `exec` result that cannot become a
/// successful [`AgentRun`].
///
/// The `codex_cli` crate reports the stream faithfully (session id, terminal
/// [`TurnVerdict`](codex_cli::TurnVerdict), usage); deciding what a failing
/// turn or a non-zero exit *means* for iter is the driver's job, so this
/// intermediate error type lives here rather than in the OSS crate. The
/// [`From`] impl below projects it onto iter's minimal domain error.
#[derive(Debug, Error)]
enum CodexOutputError {
    /// Context-window / usage-limit detected in the output.
    #[error("codex hit the usage/context limit: {0}")]
    TokenLimit(String),
    /// A terminal turn-status record reporting failure / interruption.
    #[error("codex reported turn status `{status}` (will_retry={will_retry})")]
    Reported {
        /// The status string of the failing record.
        status: String,
        /// `will_retry` flag from the accompanying error item, when present.
        will_retry: bool,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// Bad CLI arguments (clap rejection, exit `2`).
    #[error("codex rejected the command-line arguments (exit code 2)")]
    BadArgs,
    /// The process was terminated by a signal before producing a turn status.
    #[error("codex was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal turn-status record.
    #[error("codex produced no terminal turn status (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

impl From<CodexOutputError> for AgentError {
    /// Adapter projection: collapse Codex's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`CodexOutputError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; bad-args
    /// is a launch-class misconfiguration; the rest become the generic
    /// failure / signal variants.
    fn from(err: CodexOutputError) -> Self {
        match err {
            CodexOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            CodexOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
            CodexOutputError::BadArgs => {
                Self::Launch("codex rejected the command-line arguments".to_owned())
            }
            CodexOutputError::Reported {
                status,
                will_retry,
                exit_code,
            } => Self::Failed {
                code: exit_code,
                message: format!("codex reported turn status `{status}` (will_retry={will_retry})"),
            },
            CodexOutputError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "codex produced no terminal turn status".to_owned(),
            },
        }
    }
}

/// Detect a token/usage-limit in `text`: the generic context-window patterns
/// plus Codex's literal usage-limit message. Stays driver-side because
/// token-limit routing is iter-domain, not part of the CLI's own output model.
fn detect_codex_limit(text: &str) -> Option<String> {
    if let Some(detail) = detect_token_limit(text) {
        return Some(detail);
    }
    if text.contains(USAGE_LIMIT_MESSAGE) {
        return Some(USAGE_LIMIT_MESSAGE.to_owned());
    }
    None
}

/// `OpenAI` Codex driver configuration.
#[derive(Debug, Clone)]
pub struct CodexDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments inserted between the `exec` subcommand (or,
    /// in interactive mode, between the feature config override and the
    /// positional prompt.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
    /// Per-exploration hook isolation key: distinguishes one Runner's
    /// stop-hook installation from another's when both explore the same
    /// workspace path. `"default"` for standalone `iter run`.
    pub hook_isolation_key: String,
}

#[async_trait]
impl AgentDriver for CodexDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        match self.mode {
            AgentMode::Headless => {
                // `codex exec --json` from the crate, then the caller's extra
                // args, then the prompt as the final positional — preserving
                // the `exec --json [args] <prompt>` order the driver contracts
                // on. The prompt is appended after `args` so callers can still
                // extend/override the managed flags.
                let mut process = Codex::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&ExecCommand::default().json());
                for arg in &self.args {
                    process.arg(arg);
                }
                process.arg(prompt.as_str());
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "codex");
                // `codex exec` imports W3C trace context from TRACEPARENT /
                // TRACESTATE. The TUI path is not treated as verified here.
                inject_trace_context_env(&mut process);
                Ok(AgentCommand {
                    process,
                    stdin: None,
                    io: StdioMode::Piped,
                })
            }
            AgentMode::Interactive => {
                let run = RunCommand {
                    common: CommonConfig {
                        config: vec![ConfigOverride::new(
                            CODEX_HOOKS_FEATURE_CONFIG_KEY,
                            CODEX_HOOKS_FEATURE_CONFIG_VALUE,
                        )],
                        ..CommonConfig::default()
                    },
                    prompt: Some(prompt.as_str().to_owned()),
                    ..RunCommand::default()
                }
                .with_args(self.args.iter().cloned());
                let mut process = Codex::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&run);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "codex");
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
                // Parse the CLI's `exec --json` stream with the OSS crate, then
                // project its faithful verdict onto iter's domain here: a
                // terminal `Completed` turn is a run; a failing turn, a
                // usage/token limit, bad args, a signal, or the absence of any
                // terminal record each map to the matching domain error.
                let stdout = raw.stdout_str();
                let parsed = codex_cli::ExecOutput::parse(&stdout);
                let exit_code = raw.exit.exit_code();

                let Some(outcome) = parsed.turn_outcome() else {
                    // Never produced a terminal record → never ran a turn.
                    if let RawExit::Signal(sig) = raw.exit {
                        return Err(CodexOutputError::Signal(sig).into());
                    }
                    if let Some(detail) = detect_codex_limit(&stdout)
                        .or_else(|| detect_codex_limit(&raw.stderr_str()))
                    {
                        return Err(CodexOutputError::TokenLimit(detail).into());
                    }
                    if matches!(raw.exit, RawExit::Code(EXIT_BAD_ARGS)) {
                        return Err(CodexOutputError::BadArgs.into());
                    }
                    return Err(CodexOutputError::NoResult { exit_code }.into());
                };

                if outcome.status.is_completed() {
                    return Ok(AgentRun {
                        session_id: parsed.session_id(),
                    });
                }

                // A failing / interrupted turn: refine into a usage/token limit
                // when the stream text says so before reporting the raw status.
                if let Some(detail) = detect_codex_limit(&stdout) {
                    return Err(CodexOutputError::TokenLimit(detail).into());
                }
                Err(CodexOutputError::Reported {
                    status: outcome.label,
                    will_retry: outcome.will_retry,
                    exit_code,
                }
                .into())
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
        AgentKind::Codex
    }

    fn executable_read_paths(&self) -> Vec<std::path::PathBuf> {
        Codex::new(&self.command).executable_read_paths()
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

    fn codex_driver(command: impl Into<String>, mode: AgentMode) -> CodexDriver {
        CodexDriver {
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
    fn headless_command_emits_exec_json_and_inline_prompt() {
        let d = codex_driver("codex", AgentMode::Headless);
        let prompt = Prompt::from("hello-codex");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"exec".to_owned()), "got {args:?}");
        assert!(args.contains(&"--json".to_owned()), "got {args:?}");
        assert!(args.contains(&"hello-codex".to_owned()), "got {args:?}");
        assert_eq!(command.stdin, None, "codex delivers the prompt as argv");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_forwards_extra_args_before_prompt() {
        let mut d = codex_driver("codex", AgentMode::Headless);
        d.args = vec!["--model".into(), "o1".into()];
        let prompt = Prompt::from("the-prompt");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        // Argv order must be: exec --json --model o1 the-prompt
        let exec_pos = args.iter().position(|a| a == "exec").expect("exec");
        let model_pos = args.iter().position(|a| a == "--model").expect("--model");
        let prompt_pos = args
            .iter()
            .position(|a| a == "the-prompt")
            .expect("the-prompt");
        assert!(
            exec_pos < model_pos && model_pos < prompt_pos,
            "got {args:?}"
        );
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = codex_driver("codex", AgentMode::Headless);
        d.env = vec![("CODEX_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == OsStr::new("CODEX_TEST_ENV_VAR") && v == Some(OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_contains_feature_flag_pair_and_prompt_last() {
        let mut d = codex_driver("codex", AgentMode::Interactive);
        d.args = vec!["--model".into(), "gpt-5".into()];
        let prompt = Prompt::from("the-prompt");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        // Ordering: `--config` before the feature override, override before
        // extras, extras before the prompt.
        let config_pos = args
            .iter()
            .position(|a| a == "--config")
            .expect("--config present");
        let feature_config =
            format!("{CODEX_HOOKS_FEATURE_CONFIG_KEY}={CODEX_HOOKS_FEATURE_CONFIG_VALUE}");
        let feat_pos = args
            .iter()
            .position(|a| a == &feature_config)
            .expect("feature config");
        let model_pos = args.iter().position(|a| a == "--model").expect("--model");
        let prompt_pos = args.iter().position(|a| a == "the-prompt").expect("prompt");
        assert!(config_pos < feat_pos);
        assert!(feat_pos < model_pos);
        assert!(model_pos < prompt_pos);
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    // ----- interpret(): inbound translation --------------------------------

    #[test]
    fn interpret_completed_turn_extracts_session_id() {
        let d = codex_driver("codex", AgentMode::Headless);
        let stream = concat!(
            "{\"type\":\"session_configured\",\"session_id\":\"sess-x\"}\n",
            "{\"type\":\"task_complete\",\"status\":\"completed\"}\n",
        );
        let run = d
            .interpret(&synth_output(RawExit::Code(0), stream, ""))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_failed_turn_maps_to_failed_error() {
        let d = codex_driver("codex", AgentMode::Headless);
        let stream = "{\"type\":\"task_complete\",\"status\":\"failed\"}\n";
        let err = d
            .interpret(&synth_output(RawExit::Code(1), stream, ""))
            .expect_err("failed turn is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_usage_limit_maps_to_token_limit() {
        let d = codex_driver("codex", AgentMode::Headless);
        let stream = concat!(
            "{\"type\":\"error\",\"message\":\"You've hit your usage limit.\"}\n",
            "{\"type\":\"task_complete\",\"status\":\"failed\"}\n",
        );
        let err = d
            .interpret(&synth_output(RawExit::Code(1), stream, ""))
            .expect_err("usage limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_bad_args_exit_maps_to_launch() {
        let d = codex_driver("codex", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(2),
                "error: unexpected argument\n",
                "",
            ))
            .expect_err("bad args is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn interpret_interactive_judges_by_exit_only() {
        let d = codex_driver("codex", AgentMode::Interactive);
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

    /// Fake `codex` print binary: echoes each argv arg to *stderr* (so a
    /// capture sink can observe them), then prints a valid terminal
    /// turn-status JSONL stream to stdout so the driver parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s\n' '{"type":"session_configured","session_id":"sess-x"}'
printf '%s\n' '{"type":"agent_message","message":"ok"}'
printf '%s\n' '{"type":"task_complete","status":"completed"}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_argv_and_parses_session() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = codex_driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-codex");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"exec"), "got {args:?}");
        assert!(args.contains(&"--json"), "got {args:?}");
        assert!(args.contains(&"hello-codex"), "got {args:?}");
    }

    /// Fake `codex` binary for interactive mode.
    ///
    /// Invokes the installed Stop hook. The hook drains stdin and
    /// SIGKILLs `$PPID` (this fake process), causing it to exit.
    const FAKE_CODEX_SCRIPT: &str = r#"
set -uo pipefail
HOOK="$PWD/.codex/hooks/codex-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_config() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_CODEX_SCRIPT);

        let hooks_path = tmp.path().join(".codex/hooks.json");
        fs::create_dir_all(hooks_path.parent().unwrap())
            .await
            .expect("mkdir .codex");
        let user_hooks = json!({ "user_owned": true });
        fs::write(&hooks_path, serde_json::to_vec_pretty(&user_hooks).unwrap())
            .await
            .expect("write hooks.json");

        let d = codex_driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("go");
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that cleanup restored the config.
        let _ignored = drive(d, tmp.path(), &prompt).await;

        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&hooks_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_hooks,
            "user hooks.json must be restored after interactive run",
        );
        assert!(
            !tmp.path().join(".codex/hooks/codex-loop-hook.sh").exists(),
            "hook script must be cleaned up",
        );
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_cleans_up_even_when_child_fails() {
        // Fake codex that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let d = codex_driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let result = drive(d, tmp.path(), &prompt).await;

        // A non-zero exit is an `Err(Failed { code: Some(7) })`; the hook
        // bundle MUST still be cleaned up.
        let err = result.expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }
}
