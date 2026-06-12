//! [`ClineAgent`] — Cline CLI integration.
//!
//! Cline is process-restart based: each invocation runs the agent to
//! completion with no hook installation. This driver is print-only — it drives
//! the CLI's `--oneshot` mode and reads the machine-readable `--json` stream.
//!
//! # Three-layer split
//!
//! * **Command** ([`command`]) — owns the `cline --oneshot --json` argv and
//!   parses the complete output into a CLI-shaped [`command::ClineResult`] /
//!   [`command::ClineError`].
//! * **Driver/Adapter** (this module) — implements iter's [`Agent`] trait,
//!   projecting the Command result/error onto iter's domain
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
//! [`ClineAgent`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use crate::{Agent, AgentInvocation, AgentRun};
use async_trait::async_trait;

mod command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, apply_user_env, spawn_capture};
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

/// Cline CLI agent configuration.
#[derive(Debug, Clone)]
pub struct ClineAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the built-in `--oneshot --json`
    /// flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl Agent for ClineAgent {
    fn name(&self) -> &'static str {
        "cline"
    }

    fn kind(&self) -> crate::agent::AgentKind {
        crate::agent::AgentKind::Cline
    }

    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        let AgentInvocation {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            sandbox_command_prefix,
            ..
        } = ctx;

        let mut command = ClineCommand {
            program: &self.command,
            args: &self.args,
        }
        .build(workspace_path);
        apply_user_env(&mut command, &self.env);

        let output = spawn_capture(
            command,
            PromptDelivery::Stdin(prompt.as_str()),
            cancel,
            stdio_sink,
            sandbox_command_prefix,
        )
        .await?;
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<ClineError>` above.
        let result = command::interpret(&output)?;
        Ok(AgentRun {
            session_id: result.session_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Prompt;
    use crate::agent::testutil::{ctx_capturing, fake_binary_script};
    use std::path::Path;

    fn cline_agent(command: impl Into<String>) -> ClineAgent {
        ClineAgent {
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `cline` binary: echoes each argv arg and its stdin to *stderr* (so
    /// a `CaptureSink` can observe them), then prints a valid terminal
    /// `run_result` record to stdout so the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"run_result","finishReason":"completed","sessionId":"sess-x"}'"#;

    #[tokio::test]
    async fn passes_oneshot_and_json_flags_and_stdin_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = cline_agent(bin.to_string_lossy());
        let prompt = Prompt::from("hello-cline");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--oneshot"), "got {echoed:?}");
        assert!(echoed.lines().any(|l| l == "--json"), "got {echoed:?}");
        assert!(echoed.contains("hello-cline"), "got {echoed:?}");
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_after_managed_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = cline_agent(bin.to_string_lossy());
        s.args = vec!["--model".into(), "sonnet".into()];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--oneshot"), "got {args:?}");
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"sonnet"), "got {args:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$CLINE_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"type\":\"run_result\",\"finishReason\":\"completed\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = cline_agent(bin.to_string_lossy());
        s.env = vec![("CLINE_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn non_completed_run_is_an_error() {
        let script = r#"printf '%s' '{"type":"run_result","finishReason":"max_turns"}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = cline_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn no_result_on_nonzero_exit_is_an_error() {
        let (_guard, bin) = fake_binary_script("printf 'garbage\\n'\nexit 1");
        let agent = cline_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }
}
