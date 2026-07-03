//! [`ClineDriver`] — Cline CLI integration.
//!
//! Cline is process-restart based: each invocation runs the agent to
//! completion with no hook installation. This driver is print-only — it drives
//! the CLI's `--oneshot` mode and reads the machine-readable `--json` stream.
//!
//! # Two-layer split
//!
//! * **Command** ([`command`]) — owns the `cline --oneshot --json` argv and
//!   parses the complete output into a CLI-shaped [`command::ClineRun`] /
//!   [`command::ClineError`].
//! * **Driver/Adapter** (this module) — implements iter's [`AgentDriver`]
//!   trait, projecting the Command result/error onto iter's domain
//!   [`AgentRun`] / [`AgentError`] (see [`From<ClineError>`]).
//!
//! # Assumed CLI shape
//!
//! ```text
//! cline --oneshot --json [args...]
//! ```
//!
//! with the prompt on stdin. `--oneshot` runs a single turn and exits;
//! `--json` makes the terminal `run_result` record machine-readable.
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

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawOutput, apply_user_env};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

mod command;

use command::{ClineCommand, ClineError};

impl From<ClineError> for AgentError {
    /// Adapter projection: collapse Cline's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`ClineError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; the rest
    /// become the generic failure / signal variants.
    fn from(err: ClineError) -> Self {
        match err {
            ClineError::TokenLimit(detail) => Self::TokenLimit(detail),
            ClineError::Signal(sig) => Self::TerminatedBySignal(sig),
            ClineError::NotCompleted {
                finish_reason,
                exit_code,
            } => Self::Failed {
                code: exit_code,
                message: format!("cline run did not complete (finishReason `{finish_reason}`)"),
            },
            ClineError::Reported { message, exit_code } => Self::Failed {
                code: exit_code,
                message: format!("cline reported a failure event: {message}"),
            },
            ClineError::NoResult { exit_code } => Self::Failed {
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
    /// Additional arguments appended after the built-in `--oneshot --json`
    /// flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl AgentDriver for ClineDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        let mut process = ClineCommand {
            program: &self.command,
            args: &self.args,
        }
        .build(path);
        apply_user_env(&mut process, &self.env);
        Ok(AgentCommand {
            process,
            stdin: Some(prompt.as_str().to_owned()),
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<ClineError>` above.
        let result = command::interpret(RawOutput::from(output))?;
        Ok(AgentRun {
            session_id: result.session_id,
        })
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Cline
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
    use tempfile::TempDir;

    fn driver(command: impl Into<String>) -> ClineDriver {
        ClineDriver {
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

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn command_emits_oneshot_json_and_stdin_prompt() {
        let d = driver("cline");
        let prompt = Prompt::from("hello-cline");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--oneshot".to_owned()), "got {args:?}");
        assert!(args.contains(&"--json".to_owned()), "got {args:?}");
        assert_eq!(command.stdin.as_deref(), Some("hello-cline"));
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn extra_args_are_appended_after_managed_flags() {
        let mut d = driver("cline");
        d.args = vec!["--model".into(), "sonnet".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(args.contains(&"--oneshot".to_owned()), "got {args:?}");
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"sonnet".to_owned()), "got {args:?}");
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

    /// Fake `cline` binary: echoes each argv arg and its stdin to *stderr* (so
    /// the capture sink can observe them), then prints a valid terminal
    /// `run_result` record to stdout.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"run_result","finishReason":"completed","sessionId":"sess-x"}'"#;

    #[tokio::test]
    async fn oneshot_passes_through_flags_and_stdin() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("hello-cline");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--oneshot"), "got {echoed:?}");
        assert!(echoed.lines().any(|l| l == "--json"), "got {echoed:?}");
        assert!(echoed.contains("hello-cline"), "got {echoed:?}");
    }
}
