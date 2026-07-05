//! [`OpenCodeDriver`] — `OpenCode` CLI integration (print-only).
//!
//! Assembles, via the [`opencode_cli`] crate:
//!
//! ```text
//! opencode run --format json [extra-args...] <prompt>
//! ```
//!
//! The prompt is the final positional argument; `--format json` (applied by
//! the crate's [`RunCommand::json`](opencode_cli::RunCommand::json)) makes the
//! stream machine-readable. Extra args follow the managed flags — mirroring the
//! other print-only drivers — so a caller can extend the invocation. The argv
//! shape and output parsing are the crate's concern; this driver only projects
//! the crate's CLI-shaped result onto iter's domain.
//!
//! `OpenCode` is one of the **exit-0-but-failed** CLIs: the verdict lives in the
//! output stream, not the process exit code. The crate's
//! [`RunOutput`](opencode_cli::RunOutput) reports the presence of an error
//! event faithfully; this driver decides what that presence *means* for iter —
//! a generic failure, a token-limit class, or a signal.
//!
//! # `OTel`
//!
//! Unlike the other print-only drivers, this driver **does** inject
//! per-iteration `OTEL_RESOURCE_ATTRIBUTES` via
//! [`inject_agent_otel_resource_attrs`]: `OpenCode` emits its own telemetry and
//! reads that carrier before starting its spans, so tagging the resource makes
//! the agent's trace joinable with the runner's. W3C `TRACEPARENT` injection is
//! still omitted — `OpenCode`'s consumption of it is unverified, so iter does
//! not make its trace *look* correlated without confirming propagation.
//!
//! # Construction
//!
//! [`OpenCodeDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use async_trait::async_trait;
use opencode_cli::{Opencode, RunCommand, RunOutput};
use thiserror::Error;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{
    RawExit, RawOutput, apply_user_env, detect_token_limit, inject_agent_otel_resource_attrs,
};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// CLI-shaped classification of an `OpenCode` run that cannot become a
/// successful [`AgentRun`].
///
/// The `opencode_cli` crate reports the stream faithfully (session id, final
/// message, the presence of an error event); deciding what an error event or a
/// non-zero exit *means* for iter is the driver's job, so this intermediate
/// error type lives here rather than in the OSS crate. The [`From`] impl below
/// projects it onto iter's minimal domain error.
#[derive(Debug, Error)]
enum OpenCodeOutputError {
    /// Context-window / token-limit detected in an error event's message (or
    /// the surrounding stream). Router-relevant: the Adapter maps this to
    /// [`AgentError::TokenLimit`].
    #[error("opencode hit the context/token limit: {0}")]
    TokenLimit(String),
    /// An in-band `session.error` / `result.error` event was present in the
    /// output. This is the authoritative failure signal — the process may have
    /// exited `0`. `code` is the process exit code only when the process
    /// actually exited non-zero (the synchronous `result.error` path); `None`
    /// for the exit-0-but-failed path.
    #[error("opencode reported an error{}: {message}", match .code { Some(c) => format!(" (exit code {c})"), None => String::new() })]
    Failed {
        /// Process exit code, but only when the process exited non-zero.
        code: Option<i32>,
        /// The error message recovered from the event.
        message: String,
    },
    /// The process was terminated by a signal and produced no error event.
    #[error("opencode was terminated by signal {0}")]
    Signal(i32),
}

impl From<OpenCodeOutputError> for AgentError {
    /// Adapter projection: collapse `OpenCode`'s CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`OpenCodeOutputError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; a reported
    /// error event becomes [`AgentError::Failed`] (carrying the exit code only
    /// when the process actually exited non-zero), and a terminating signal
    /// becomes [`AgentError::TerminatedBySignal`].
    fn from(err: OpenCodeOutputError) -> Self {
        match err {
            OpenCodeOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            OpenCodeOutputError::Failed { code, message } => Self::Failed { code, message },
            OpenCodeOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
        }
    }
}

/// `OpenCode` CLI driver configuration.
#[derive(Debug, Clone)]
pub struct OpenCodeDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments inserted after the managed `run --format json`
    /// flags and before the positional prompt.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl AgentDriver for OpenCodeDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // `opencode run --format json` from the crate, then the caller's extra
        // args, then the prompt as the final positional. The prompt is appended
        // after `args` so callers can still extend the managed invocation.
        let mut process = Opencode::new(&self.command)
            .with_current_dir(path)
            .to_process(&RunCommand::default().json());
        for arg in &self.args {
            process.arg(arg);
        }
        process.arg(prompt.as_str());
        apply_user_env(&mut process, &self.env);
        inject_agent_otel_resource_attrs(&mut process, path, "opencode");
        // Trace-context env (W3C `TRACEPARENT`) injection is deliberately
        // omitted: `OpenCode`'s consumption of it is unverified, and injecting a
        // carrier would make the agent's trace *look* correlated without it
        // actually participating in propagation.

        // The prompt is embedded in the argv, so no stdin payload is sent.
        Ok(AgentCommand {
            process,
            stdin: None,
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // Parse the CLI's `run --format json` stream with the OSS crate, then
        // project its faithful verdict onto iter's domain here. `?`/`.into()`
        // run the `From<OpenCodeOutputError>` above.
        let raw = RawOutput::from(output);
        let stdout = raw.stdout_str();
        let parsed = RunOutput::parse(&stdout);
        let exit_code = raw.exit.exit_code();

        // Presence of an error event is authoritative — even on exit 0.
        if let Some(error) = parsed.error() {
            let message = error.into_message();
            // Refine into TokenLimit when the message — or anywhere in the
            // stream — describes a context/token limit.
            if let Some(detail) =
                detect_token_limit(&message).or_else(|| detect_token_limit(&stdout))
            {
                return Err(OpenCodeOutputError::TokenLimit(detail).into());
            }
            // Carry the exit code only when the process actually exited
            // non-zero (the synchronous `result.error` path); the
            // exit-0-but-failed path reports `None`.
            let code = exit_code.filter(|&c| c != 0);
            let message = if message.is_empty() {
                "opencode reported an error event".to_owned()
            } else {
                message
            };
            return Err(OpenCodeOutputError::Failed { code, message }.into());
        }

        // No error event. A terminating signal with no in-band error is a
        // process-level termination.
        if let RawExit::Signal(sig) = raw.exit {
            return Err(OpenCodeOutputError::Signal(sig).into());
        }

        // A non-zero exit with NO in-band error event is a pre-flight /
        // validation failure that crashed before OpenCode could write a
        // `result.error`. The exit-0-but-failed path is already handled above
        // by the error-event check, so trusting a non-zero exit here is sound —
        // it is the only signal a never-emitted-JSON crash leaves behind.
        if let RawExit::Code(code) = raw.exit
            && code != 0
        {
            return Err(OpenCodeOutputError::Failed {
                code: Some(code),
                message: format!("opencode exited with code {code} and no result event"),
            }
            .into());
        }

        // Success: recover any session id from the stream.
        Ok(AgentRun {
            session_id: parsed.session_id(),
        })
    }

    fn kind(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    fn executable_read_paths(&self) -> Vec<std::path::PathBuf> {
        Opencode::new(&self.command).executable_read_paths()
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RawExit;
    use crate::agent::testutil::{drive_capturing, fake_binary_script};
    use std::ffi::OsStr;
    use tempfile::TempDir;

    fn driver(command: impl Into<String>) -> OpenCodeDriver {
        OpenCodeDriver {
            command: command.into(),
            args: Vec::new(),
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

    fn env_of(command: &AgentCommand, key: &str) -> Option<String> {
        command
            .process
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn command_emits_run_json_and_inline_prompt() {
        let d = driver("opencode");
        let prompt = Prompt::from("hello-opencode");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert_eq!(
            args.first().map(String::as_str),
            Some("run"),
            "got {args:?}"
        );
        assert!(args.contains(&"--format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        assert!(args.contains(&"hello-opencode".to_owned()), "got {args:?}");
        assert_eq!(command.stdin, None, "prompt is inline, not stdin");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn extra_args_are_forwarded_before_the_prompt() {
        let mut d = driver("opencode");
        d.args = vec!["--model".into(), "sonnet".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        // Order: run --format json --model sonnet x
        let model_pos = args.iter().position(|a| a == "--model").expect("--model");
        let prompt_pos = args.iter().position(|a| a == "x").expect("prompt");
        assert!(args.contains(&"--format".to_owned()), "got {args:?}");
        assert!(args.contains(&"sonnet".to_owned()), "got {args:?}");
        assert!(
            model_pos < prompt_pos,
            "extras precede the prompt: {args:?}"
        );
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("opencode");
        d.env = vec![("OPENCODE_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            env_of(&command, "OPENCODE_TEST_ENV_VAR").as_deref(),
            Some("env-value"),
            "declared env must be applied to the child command",
        );
    }

    #[test]
    fn command_injects_agent_otel_resource_attrs() {
        let d = driver("opencode");
        let prompt = Prompt::from("x");
        let tmp = TempDir::new().expect("tmp");
        let command = d.command(tmp.path(), &prompt, None).expect("command");
        let attrs = env_of(&command, "OTEL_RESOURCE_ATTRIBUTES")
            .expect("OTEL_RESOURCE_ATTRIBUTES must be injected");
        assert!(
            attrs.contains("iter.agent.driver=opencode"),
            "got {attrs:?}",
        );
    }

    // ----- interpret(): inbound projection onto the domain ------------------

    #[test]
    fn interpret_clean_session_extracts_session_id() {
        let d = driver("opencode");
        let body = r#"{"type":"session","id":"sess-x","status":"idle"}"#;
        let run = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_session_error_on_exit_zero_is_a_failure() {
        // `OpenCode` exits 0 even on failure — the error event is authoritative.
        let d = driver("opencode");
        let body = r#"{"type":"session.error","error":{"message":"auth failed"}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: None, ref message } if message == "auth failed"),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_result_error_on_exit_one_carries_the_exit_code() {
        let d = driver("opencode");
        let body = r#"{"type":"result.error","error":{"message":"bad flag"}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), ref message } if message == "bad flag"),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_error_event_maps_to_token_limit() {
        let d = driver("opencode");
        let body = r#"{"type":"session.error","error":{"message":"context window exceeded"}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect_err("must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_nonzero_exit_without_error_event_is_a_failure() {
        // A pre-flight/validation crash exits non-zero before writing any
        // `result.error` JSON; the exit code is the only signal left.
        let d = driver("opencode");
        let err = d
            .interpret(&synth_output(RawExit::Code(1), ""))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_signal_without_error_event_maps_to_signal() {
        let d = driver("opencode");
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), ""))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}"
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `opencode` binary: echoes each argv arg (one per line) to *stderr*
    /// so the capture sink can observe the flags, then prints a clean session
    /// record to stdout so the crate parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"type":"session","id":"sess-x","status":"idle"}'"#;

    #[tokio::test]
    async fn run_passes_subcommand_and_inline_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("hello-opencode");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert_eq!(args.first(), Some(&"run"), "got {args:?}");
        assert!(args.contains(&"hello-opencode"), "got {args:?}");
    }
}
