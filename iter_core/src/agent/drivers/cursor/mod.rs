//! [`CursorAgent`] — Cursor `cursor-agent` CLI integration.
//!
//! Cursor's CLI is process-restart based: it has no hook plumbing and runs
//! to completion on each invocation. This agent is therefore **print-only**
//! — there is no interactive/TUI mode distinction.
//!
//! # Assumed CLI shape
//!
//! ```text
//! cursor-agent --print --output-format json [args...]
//! ```
//!
//! with the prompt written to stdin. `--print` causes the binary to emit a
//! single response and exit; `--output-format json` makes the terminal
//! `result` record machine-readable so the driver can recover the session id.
//!
//! The per-CLI argv construction and output parsing — including the subtle
//! success contract (presence of a terminal `result` record, *not* the
//! hard-coded `is_error` field) — live in the [`command`] submodule, the
//! **Command level** of the agent stack. This module is the Adapter: it wires
//! the Command to the shared spawn primitive and projects the Command's
//! CLI-shaped result/error onto iter's domain
//! [`AgentRun`] / [`AgentError`].
//!
//! # Construction
//!
//! [`CursorAgent`] exposes no defaults. Every field on [`CursorSettings`] is
//! required because the value is a project-shaped decision iter cannot
//! honestly pick on the operator's behalf.

use crate::{Agent, AgentRun, AgentRunContext};

mod command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, apply_user_env, spawn_capture};
use command::{CursorCommand, CursorError};

impl From<CursorError> for AgentError {
    /// Adapter projection: collapse Cursor's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`CursorError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`];
    /// [`CursorError::BelowMinVersion`] is a startup failure that never ran a
    /// turn, so it maps to [`AgentError::Launch`]; the rest become the
    /// generic failure / signal variants.
    fn from(err: CursorError) -> Self {
        match err {
            CursorError::TokenLimit(detail) => Self::TokenLimit(detail),
            CursorError::Signal(sig) => Self::TerminatedBySignal(sig),
            CursorError::BelowMinVersion => Self::Launch(
                "cursor-agent is below the minimum supported version (exit 2)".to_owned(),
            ),
            CursorError::NoResult { exit_code, detail } => Self::Failed {
                code: exit_code,
                message: format!("cursor-agent produced no terminal result: {detail}"),
            },
        }
    }
}

/// Fully-specified configuration for [`CursorAgent`].
#[derive(Debug, Clone)]
pub struct CursorSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Additional arguments appended after the built-in print flags. Empty is
    /// allowed.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

/// Cursor `cursor-agent` CLI agent configuration.
#[derive(Debug, Clone)]
pub struct CursorAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the built-in print flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl CursorAgent {
    /// Build a fully-specified Cursor agent.
    #[must_use]
    pub fn new(settings: CursorSettings) -> Self {
        let CursorSettings { command, args, env } = settings;
        Self { command, args, env }
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    #[must_use]
    pub fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }
}

impl Agent for CursorAgent {
    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            ..
        } = ctx;

        let mut command = CursorCommand {
            program: &self.command,
            args: &self.args,
        }
        .build(workspace_path);
        apply_user_env(&mut command, &self.env);
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted: cursor-agent's consumption of `TRACEPARENT` /
        // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so — like the other
        // print-only drivers — iter does not make its traces *look*
        // correlated without confirming the agent actually participates.

        let output = spawn_capture(
            command,
            PromptDelivery::Stdin(prompt.as_str()),
            cancel,
            stdio_sink,
        )
        .await?;
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<CursorError>` above.
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

    fn settings(command: impl Into<String>) -> CursorSettings {
        CursorSettings {
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `cursor-agent` print binary: echoes each argv arg and its stdin to
    /// *stderr* (so a [`crate::agent::testutil::CaptureSink`] can observe
    /// them), then prints a valid terminal `result` JSON object to stdout so
    /// the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x","request_id":"req-x"}'"#;

    #[tokio::test]
    async fn passes_print_flag_and_stdin_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = CursorAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("hello-cursor");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--print"), "got {echoed:?}");
        assert!(echoed.contains("hello-cursor"), "got {echoed:?}");
    }

    #[tokio::test]
    async fn emits_output_format_json() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = CursorAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_after_print_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = settings(bin.to_string_lossy());
        s.args = vec!["--model".into(), "sonnet".into()];
        let agent = CursorAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--print"), "got {args:?}");
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"sonnet"), "got {args:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$CURSOR_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"type\":\"result\",\"is_error\":false,\"session_id\":\"s\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = settings(bin.to_string_lossy());
        s.env = vec![("CURSOR_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = CursorAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn no_terminal_result_maps_to_failed() {
        // Exits non-zero and emits no `result` record → Adapter maps the
        // Command's `NoResult` onto `AgentError::Failed`.
        let (_guard, bin) = fake_binary_script("printf 'boom\\n' 1>&2; exit 1");
        let agent = CursorAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("nonzero without result is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn token_limit_maps_to_token_limit() {
        let (_guard, bin) =
            fake_binary_script("printf 'context window exceeded\\n' 1>&2; exit 1");
        let agent = CursorAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn below_min_version_maps_to_launch() {
        let (_guard, bin) = fake_binary_script("exit 2");
        let agent = CursorAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("exit 2 is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }
}
