//! [`OpenCodeDriver`] — `OpenCode` CLI integration (print-only).
//!
//! Assembles:
//!
//! ```text
//! opencode run [args...] --format json <prompt>
//! ```
//!
//! The prompt is the final positional argument; `--format json` makes the
//! stream machine-readable. The argv shape and output-parsing live at the
//! Command level (`command.rs`); this driver only projects the Command's
//! CLI-shaped result/error onto iter's domain.
//!
//! `OpenCode` is one of the **exit-0-but-failed** CLIs: the verdict lives in the
//! output stream, not the process exit code. See `command.rs` for the full
//! contract.
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

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawOutput, apply_user_env, inject_agent_otel_resource_attrs};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

mod command;

use command::{OpenCodeCommand, OpenCodeError};

impl From<OpenCodeError> for AgentError {
    /// Adapter projection: collapse `OpenCode`'s CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`OpenCodeError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; a reported
    /// error event becomes [`AgentError::Failed`] (carrying the exit code only
    /// when the process actually exited non-zero), and a terminating signal
    /// becomes [`AgentError::TerminatedBySignal`].
    fn from(err: OpenCodeError) -> Self {
        match err {
            OpenCodeError::TokenLimit(detail) => Self::TokenLimit(detail),
            OpenCodeError::Failed { code, message } => Self::Failed { code, message },
            OpenCodeError::Signal(sig) => Self::TerminatedBySignal(sig),
        }
    }
}

/// `OpenCode` CLI driver configuration.
#[derive(Debug, Clone)]
pub struct OpenCodeDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments inserted between the `run` subcommand and the
    /// managed `--format json` flag.
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
        let mut process = OpenCodeCommand {
            program: &self.command,
            args: &self.args,
            prompt: prompt.as_str(),
        }
        .build(path);
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
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<OpenCodeError>` above.
        let result = command::interpret(RawOutput::from(output))?;
        Ok(AgentRun {
            session_id: result.session_id,
        })
    }

    fn kind(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
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
    fn extra_args_are_forwarded_before_format_flag() {
        let mut d = driver("opencode");
        d.args = vec!["--model".into(), "sonnet".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"sonnet".to_owned()), "got {args:?}");
        assert!(args.contains(&"--format".to_owned()), "got {args:?}");
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
    fn interpret_token_limit_error_event_maps_to_token_limit() {
        let d = driver("opencode");
        let body = r#"{"type":"session.error","error":{"message":"context window exceeded"}}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect_err("must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `opencode` binary: echoes each argv arg (one per line) to *stderr*
    /// so the capture sink can observe the flags, then prints a clean session
    /// record to stdout so the Command parses an `Ok`.
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
