//! [`GenericAgent`] — run any configured CLI command as an agent.
//!
//! Used as an escape hatch when none of the first-class integrations fit: if
//! you have a command-line tool that consumes a prompt and writes back to
//! stdout, `GenericAgent` can drive it.

use std::path::Path;

use crate::{Agent, AgentInvocation, AgentRun, Prompt};
use async_trait::async_trait;
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, detect_token_limit, spawn_capture};

/// Runs any configured CLI command as an [`Agent`].
///
/// [`GenericAgent::command`] is an `argv` vector. If [`stdin_prompt`] is
/// `true`, the prompt is written to the child's stdin; otherwise it is
/// appended as the last positional argument.
///
/// # Example
///
/// ```no_run
/// use iter_core::agent::GenericAgent;
/// use iter_core::{Agent, AgentInvocation, Prompt};
/// use iter_core::signal::SignalId;
/// use std::path::Path;
/// use tokio_util::sync::CancellationToken;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "cat".into()]);
/// let prompt = Prompt::from("hello");
/// let ctx = AgentInvocation::new(
///     Path::new("."),
///     &prompt,
///     CancellationToken::new(),
///     SignalId::new(),
/// );
/// // `Ok` means the command ran a turn; a non-zero exit is `Err`.
/// let _run = agent.run(ctx).await?;
/// # Ok(()) }
/// ```
///
/// [`stdin_prompt`]: GenericAgent::stdin_prompt
#[derive(Debug, Clone, Default)]
pub struct GenericAgent {
    /// The full argv vector used to spawn the child.
    pub command: Vec<String>,
    /// When `true`, the prompt is written to the child's stdin. Otherwise it
    /// is appended as the final positional argument.
    pub stdin_prompt: bool,
    /// Extra environment variables applied to the child process.
    pub env: Vec<(String, String)>,
}

impl GenericAgent {
    /// Construct a new [`GenericAgent`] with the given argv.
    ///
    /// Defaults: prompt is delivered on stdin, no extra env vars.
    #[must_use]
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            stdin_prompt: true,
            env: Vec::new(),
        }
    }

    /// Toggle whether the prompt should be delivered on stdin (`true`) or
    /// appended as the final argv entry (`false`).
    #[must_use]
    pub fn with_stdin_prompt(mut self, stdin_prompt: bool) -> Self {
        self.stdin_prompt = stdin_prompt;
        self
    }

    /// Append an environment variable to the child's env.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn build_command(&self, path: &Path, prompt: &Prompt) -> Result<Command, AgentError> {
        let (program, rest) = self
            .command
            .split_first()
            .ok_or_else(|| AgentError::Launch("agent command is empty".to_owned()))?;
        let mut cmd = Command::new(program);
        cmd.current_dir(path);
        for arg in rest {
            cmd.arg(arg);
        }
        if !self.stdin_prompt {
            cmd.arg(prompt.as_str());
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        Ok(cmd)
    }
}

#[async_trait]
impl Agent for GenericAgent {
    fn name(&self) -> &'static str {
        "generic"
    }

    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        let command = self.build_command(ctx.workspace_path, ctx.prompt)?;
        let delivery = if self.stdin_prompt {
            PromptDelivery::Stdin(ctx.prompt.as_str())
        } else {
            PromptDelivery::Inline
        };
        // The generic escape hatch has no machine-readable contract: a clean
        // exit is a run, a non-zero exit is a failure. Token-limit text in
        // the output is still surfaced so the router can fall back.
        let output = spawn_capture(
            command,
            delivery,
            ctx.cancel,
            ctx.stdio_sink,
            ctx.sandbox_command_prefix,
        )
        .await?;
        if let Some(err) = output.exit.into_failure() {
            if let Some(detail) = detect_token_limit(&output.stdout_str())
                .or_else(|| detect_token_limit(&output.stderr_str()))
            {
                return Err(AgentError::TokenLimit(detail));
            }
            return Err(err);
        }
        Ok(AgentRun::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{ctx, ctx_capturing};

    #[tokio::test]
    async fn captures_stdout_on_success() {
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "echo hello".into()]);
        let prompt = Prompt::from("ignored");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stdout().await.contains("hello"));
    }

    #[tokio::test]
    async fn non_zero_exit_reported_as_failure() {
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "exit 7".into()]);
        let prompt = Prompt::from("ignored");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn stdin_prompt_is_piped_to_child() {
        // `cat` with no args copies stdin to stdout; assert we see the prompt.
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "cat".into()]);
        let prompt = Prompt::from("from-stdin");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stdout().await.contains("from-stdin"));
    }

    #[tokio::test]
    async fn inline_prompt_is_appended_as_arg() {
        // `sh -c 'printf %s "$1"' placeholder <prompt>` — the runtime appends
        // the prompt as the next positional, observable as `$1`.
        let agent = GenericAgent::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$1\"".into(),
            "placeholder".into(),
        ])
        .with_stdin_prompt(false);
        let prompt = Prompt::from("appended-arg");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert_eq!(sink.stdout().await, "appended-arg");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let agent = GenericAgent::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$ITER_TEST_VAR\"".into(),
        ])
        .with_env("ITER_TEST_VAR", "env-value");
        let prompt = Prompt::from("ignored");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert_eq!(sink.stdout().await, "env-value");
    }

    #[tokio::test]
    async fn empty_command_errors() {
        let agent = GenericAgent::new(Vec::new());
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("must fail");
        assert!(matches!(err, AgentError::Launch(_)));
    }

    #[tokio::test]
    async fn dsl_env_is_forwarded_to_child() {
        let mut agent = GenericAgent::new(vec![
            "sh".into(),
            "-c".into(),
            "printf '%s %s' \"$DSL_VAR_A\" \"$DSL_VAR_B\"".into(),
        ]);
        agent.env = vec![
            ("DSL_VAR_A".into(), "alpha".into()),
            ("DSL_VAR_B".into(), "beta".into()),
        ];
        let prompt = Prompt::from("ignored");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert_eq!(sink.stdout().await, "alpha beta");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_termination_is_reported() {
        // Child kills itself with SIGKILL (signal 9). On Unix, the parent
        // should see `AgentError::TerminatedBySignal(9)`.
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "kill -KILL $$".into()]);
        let prompt = Prompt::from("ignored");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("signal is an error");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn working_directory_is_applied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "pwd".into()]);
        let prompt = Prompt::from("ignored");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");
        let out = sink.stdout().await;
        // Resolve the canonical path to avoid symlink mismatches on macOS.
        let canonical = tmp.path().canonicalize().expect("canonicalize");
        assert!(
            out.contains(canonical.to_string_lossy().as_ref()),
            "expected {canonical:?} in {out:?}"
        );
    }
}
