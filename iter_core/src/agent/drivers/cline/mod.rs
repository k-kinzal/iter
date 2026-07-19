//! [`ClineDriver`] — Cline CLI integration.
//!
//! Cline is process-restart based: each invocation runs the agent to
//! completion with no hook installation. This driver is print-only — it runs a
//! single prompt and reads the machine-readable `--json` NDJSON stream.
//!
//! # Two-layer split
//!
//! * **Command** ([`cline_cli`]) — the standalone `cline_cli` crate owns the
//!   `cline --json <prompt>` argv and models the NDJSON run stream as a
//!   [`RunOutput`] with typed accessors (terminal `run_result`, `run_aborted`,
//!   `error`).
//! * **Driver/Adapter** (this module) — implements iter's [`AgentDriver`]
//!   trait, projecting the crate's output onto iter's domain [`AgentRun`] /
//!   [`AgentError`] (see [`ClineOutputError`] and its [`From`] impl).
//!
//! # Assumed CLI shape
//!
//! ```text
//! cline --json <prompt> [args...]
//! ```
//!
//! The prompt is a **positional argument**. Cline `3.0.23` has no `--oneshot`
//! flag and reads nothing from stdin; `--json` makes the terminal `run_result`
//! record machine-readable. Caller-supplied `args` are appended after the
//! prompt.
//!
//! # Output contract (Cline CLI, `--json`)
//!
//! The stream is NDJSON: any number of progress / error events followed by a
//! terminal `run_result` record. The records iter keys off:
//!
//! ```jsonc
//! { "type": "run_result", "finishReason": "completed", "sessionId": "<id>",
//!   "message": "<final assistant message>" }
//! { "type": "run_aborted", "reason": "..." }
//! { "type": "error", "message": "..." }
//! ```
//!
//! Field → conclusion chain: *did it run* = a `run_result` record is present;
//! *success/fail* = `finishReason == "completed"`; *why* = any other
//! `finishReason`, a `run_aborted` record, or an `error` event. The terminal
//! record is authoritative; the exit code is only consulted when no record was
//! produced (a Commander argument-parse error can leak exit `0`).
//!
//! # `OTel`
//!
//! Like the other print-only drivers, `OTel` trace-context / resource-attribute
//! injection is deliberately omitted: Cline's consumption of `TRACEPARENT` /
//! `OTEL_RESOURCE_ATTRIBUTES` is unverified, so iter does not make its traces
//! *look* correlated without confirming the agent actually participates.
//!
//! # Construction
//!
//! [`ClineDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use async_trait::async_trait;
use cline_cli::{Cline, RunCommand, RunOutput};
use thiserror::Error;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawExit, RawOutput, apply_user_env, detect_token_limit};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// CLI-shaped error hierarchy for Cline, projected onto [`AgentError`] by the
/// [`From`] impl below.
#[derive(Debug, Error)]
enum ClineOutputError {
    /// Context-window / token-limit detected in the output.
    #[error("cline hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A terminal `run_result` record whose `finishReason` was not
    /// `completed`.
    #[error("cline run did not complete (finishReason `{finish_reason}`)")]
    NotCompleted {
        /// The `finishReason` of the failing record.
        finish_reason: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// A `run_aborted` record, or an `error` event, surfaced before any
    /// terminal `run_result`.
    #[error("cline reported a failure event: {message}")]
    Reported {
        /// Short human-readable summary read from the event.
        message: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("cline was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal `run_result`
    /// record.
    #[error("cline produced no run_result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

impl From<ClineOutputError> for AgentError {
    /// Adapter projection: collapse Cline's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`ClineOutputError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; the rest
    /// become the generic failure / signal variants.
    fn from(err: ClineOutputError) -> Self {
        match err {
            ClineOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            ClineOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
            ClineOutputError::NotCompleted {
                finish_reason,
                exit_code,
            } => Self::Failed {
                code: exit_code,
                message: format!("cline run did not complete (finishReason `{finish_reason}`)"),
            },
            ClineOutputError::Reported { message, exit_code } => Self::Failed {
                code: exit_code,
                message: format!("cline reported a failure event: {message}"),
            },
            ClineOutputError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "cline produced no run_result".to_owned(),
            },
        }
    }
}

/// Cline CLI driver configuration.
#[derive(Debug, Clone)]
pub struct ClineDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the managed `--json <prompt>`.
    pub args: Vec<String>,
    /// Optional replacement for Cline's default system prompt.
    pub system_prompt: Option<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl ClineDriver {
    /// Classify Cline's complete `--json` output into a run or an error.
    ///
    /// The terminal `run_result` record is authoritative for *did it run*; the
    /// exit code is only consulted when no record was produced.
    fn classify(raw: &RawOutput<'_>) -> Result<AgentRun, AgentError> {
        let stdout = raw.stdout_str();
        let exit_code = raw.exit.exit_code();
        let parsed = RunOutput::parse(&stdout);

        // The terminal `run_result` record is authoritative for *did it run*.
        if let Some(result) = parsed.run_result() {
            if result.finish_reason.is_completed() {
                return Ok(AgentRun {
                    session_id: result.session_id,
                });
            }
            // Ran a turn but did not complete — refine into token-limit when
            // the stream text says so, otherwise report the finish reason.
            if let Some(detail) = result
                .message
                .as_deref()
                .and_then(detect_token_limit)
                .or_else(|| detect_token_limit(&stdout))
            {
                return Err(ClineOutputError::TokenLimit(detail).into());
            }
            return Err(ClineOutputError::NotCompleted {
                finish_reason: result.finish_reason.as_str().to_owned(),
                exit_code,
            }
            .into());
        }

        // No terminal record. Run token-limit detection over the stream first
        // so a context-window failure is classified before the event paths.
        if let Some(detail) = detect_token_limit(&stdout) {
            return Err(ClineOutputError::TokenLimit(detail).into());
        }
        let stderr = raw.stderr_str();
        if let Some(detail) = detect_token_limit(&stderr) {
            return Err(ClineOutputError::TokenLimit(detail).into());
        }

        // A `run_aborted` record or an `error` event explains the failure.
        if let Some(message) = parsed.failure_message() {
            return Err(ClineOutputError::Reported { message, exit_code }.into());
        }

        // Nothing in-band: a signal is process-level termination; any other
        // disposition is a no-result failure carrying whatever exit surfaced.
        if let RawExit::Signal(sig) = raw.exit {
            return Err(ClineOutputError::Signal(sig).into());
        }
        Err(ClineOutputError::NoResult { exit_code }.into())
    }
}

#[async_trait]
impl AgentDriver for ClineDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // Argv construction is delegated to `cline_cli`: `--json` selects the
        // NDJSON run stream and the prompt is the trailing positional argument.
        // Cline `3.0.23` has no `--oneshot` flag and reads nothing from stdin,
        // so nothing is fed on stdin. The caller's extra args are appended last.
        let mut run = RunCommand::prompt(prompt.as_str());
        run.options.system.clone_from(&self.system_prompt);
        let run = run.json();
        let mut process = Cline::new(&self.command)
            .with_current_dir(path)
            .to_process(&run);
        process.args(&self.args);
        apply_user_env(&mut process, &self.env);
        Ok(AgentCommand {
            process,
            stdin: None,
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // Adapter: project the crate's CLI-shaped output onto iter's domain.
        Self::classify(&RawOutput::from(output))
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Cline
    }

    fn executable_read_paths(&self) -> Vec<std::path::PathBuf> {
        Cline::new(&self.command).executable_read_paths()
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{drive_capturing, fake_binary_script};
    use tempfile::TempDir;

    fn driver(command: impl Into<String>) -> ClineDriver {
        ClineDriver {
            command: command.into(),
            args: Vec::new(),
            system_prompt: None,
            env: Vec::new(),
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

    fn synth_output(exit: RawExit, stdout: &str) -> std::process::Output {
        std::process::Output {
            status: exit.into_exit_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn command_emits_json_and_positional_prompt() {
        // Correctness fix: Cline 3.0.23 has no `--oneshot` flag and takes the
        // prompt as a positional argument, not on stdin. The argv is now built
        // by `cline_cli`, which renders `--json <prompt>`.
        let d = driver("cline");
        let prompt = Prompt::from("hello-cline");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            argv(&command),
            vec!["--json".to_owned(), "hello-cline".to_owned()]
        );
        assert!(
            !argv(&command).contains(&"--oneshot".to_owned()),
            "cline 3.0.23 has no --oneshot flag",
        );
        assert_eq!(command.stdin, None, "the prompt is a positional, not stdin");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn extra_args_are_appended_after_the_prompt() {
        // The prompt is the trailing positional of the managed argv; extra args
        // follow it (Commander accepts options after positionals).
        let mut d = driver("cline");
        d.args = vec!["--model".into(), "sonnet".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(
            args,
            vec![
                "--json".to_owned(),
                "x".to_owned(),
                "--model".to_owned(),
                "sonnet".to_owned(),
            ],
        );
    }

    #[test]
    fn system_prompt_is_forwarded_before_the_task_prompt() {
        let mut d = driver("cline");
        d.system_prompt = Some("Use read-only tools.".into());
        let prompt = Prompt::from("inspect");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(
            args,
            vec![
                "--json".to_owned(),
                "--system".to_owned(),
                "Use read-only tools.".to_owned(),
                "inspect".to_owned(),
            ],
        );
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("cline");
        d.env = vec![("CLINE_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("CLINE_TEST_ENV_VAR")
                && v == Some(std::ffi::OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    // ----- interpret(): inbound projection onto the domain ------------------

    #[test]
    fn interpret_completed_run_extracts_session_id() {
        let d = driver("cline");
        let body = r#"{"type":"run_result","finishReason":"completed","sessionId":"sess-x"}"#;
        let run = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_non_completed_run_maps_to_failed() {
        let d = driver("cline");
        let body = r#"{"type":"run_result","finishReason":"max_turns"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_in_run_result_message_classifies() {
        let d = driver("cline");
        let body =
            r#"{"type":"run_result","finishReason":"error","message":"context window exceeded"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_run_aborted_maps_to_failed_with_reason() {
        let d = driver("cline");
        let body = r#"{"type":"run_aborted","reason":"user cancelled"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { ref message, .. } if message.contains("user cancelled")),
            "got {err:?}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn interpret_signal_termination_survives() {
        let d = driver("cline");
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), ""))
            .expect_err("signal must fail");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_no_result_maps_to_failed() {
        let d = driver("cline");
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "garbage"))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `cline` binary: echoes each argv arg to *stderr* (so the capture
    /// sink can observe them), then prints a valid terminal `run_result` record
    /// to stdout.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"type":"run_result","finishReason":"completed","sessionId":"sess-x"}'"#;

    #[tokio::test]
    async fn run_passes_json_and_positional_prompt_through() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("hello-cline");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        let json_pos = args.iter().position(|a| *a == "--json").expect("--json");
        let prompt_pos = args
            .iter()
            .position(|a| *a == "hello-cline")
            .expect("prompt");
        assert!(json_pos < prompt_pos, "got {args:?}");
        assert!(!args.contains(&"--oneshot"), "no --oneshot flag: {args:?}");
    }
}
