//! [`OpenCodeAgent`] — `OpenCode` CLI integration (print-only).
//!
//! Spawns:
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
//! # Construction
//!
//! [`OpenCodeAgent`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use crate::{Agent, AgentRun, AgentRunContext};
use async_trait::async_trait;

use crate::agent::AgentError;
use crate::agent::process::{
    PromptDelivery, apply_user_env, inject_agent_otel_resource_attrs, spawn_capture,
};

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

/// `OpenCode` CLI agent configuration.
#[derive(Debug, Clone)]
pub struct OpenCodeAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments inserted between the `run` subcommand and the
    /// managed `--format json` flag.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl OpenCodeAgent {
    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    #[must_use]
    pub fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }
}

#[async_trait]
impl Agent for OpenCodeAgent {
    fn name(&self) -> &'static str {
        "opencode"
    }

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            sandbox_command_prefix,
            ..
        } = ctx;

        let mut command = OpenCodeCommand {
            program: &self.command,
            args: &self.args,
            prompt: prompt.as_str(),
        }
        .build(workspace_path);
        apply_user_env(&mut command, &self.env);
        inject_agent_otel_resource_attrs(
            &mut command,
            signal_id,
            signal_kind,
            workspace_path,
            "opencode",
        );
        // Trace-context env (W3C `TRACEPARENT`) injection is deliberately
        // omitted: `OpenCode`'s consumption of it is unverified, and injecting a
        // carrier would make the agent's trace *look* correlated without it
        // actually participating in propagation.

        // The prompt is embedded in the argv, so no stdin payload is sent.
        let output = spawn_capture(
            command,
            PromptDelivery::Inline,
            cancel,
            stdio_sink,
            sandbox_command_prefix,
        )
        .await?;
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<OpenCodeError>` above.
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
    use tempfile::TempDir;

    fn opencode_agent(command: impl Into<String>) -> OpenCodeAgent {
        OpenCodeAgent {
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `opencode` binary: echoes each argv arg (one per line) to *stderr*
    /// so a [`CaptureSink`] can observe the flags, then prints a clean session
    /// record to stdout so the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"type":"session","id":"sess-x","status":"idle"}'"#;

    #[tokio::test]
    async fn passes_run_subcommand_and_inline_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = opencode_agent(bin.to_string_lossy());
        let prompt = Prompt::from("hello-opencode");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert_eq!(args.first(), Some(&"run"), "got {args:?}");
        assert!(args.contains(&"hello-opencode"), "got {args:?}");
    }

    #[tokio::test]
    async fn requests_json_format() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = opencode_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_before_format_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = opencode_agent(bin.to_string_lossy());
        s.args = vec!["--model".into(), "sonnet".into()];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"sonnet"), "got {args:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$OPENCODE_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"type\":\"session\",\"id\":\"s\",\"status\":\"idle\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = opencode_agent(bin.to_string_lossy());
        s.env = vec![("OPENCODE_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn injects_signal_resource_attributes() {
        let (_guard, bin) = fake_binary_script(
            "printf '%s' \"$OTEL_RESOURCE_ATTRIBUTES\" 1>&2\nprintf '%s' '{\"type\":\"session\",\"id\":\"s\",\"status\":\"idle\"}'",
        );
        let tmp = TempDir::new().expect("tmp");
        let agent = opencode_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        assert!(echoed.contains("iter.signal.id="), "got {echoed:?}");
        assert!(echoed.contains("iter.signal.kind=work"), "got {echoed:?}");
        assert!(
            echoed.contains("iter.agent.driver=opencode"),
            "got {echoed:?}"
        );
    }

    #[tokio::test]
    async fn session_error_on_exit_zero_is_a_failure() {
        // `OpenCode` exits 0 even on failure — the error event is authoritative.
        let script = r#"printf '%s' '{"type":"session.error","error":{"message":"auth failed"}}'"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = opencode_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: None, ref message } if message == "auth failed"),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn token_limit_error_event_maps_to_token_limit() {
        let script = r#"printf '%s' '{"type":"session.error","error":{"message":"context window exceeded"}}'"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = opencode_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }
}
