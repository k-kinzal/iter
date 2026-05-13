//! [`GenericAgent`] — run any configured CLI command as an agent.
//!
//! Used as an escape hatch when none of the first-class integrations fit: if
//! you have a command-line tool that consumes a prompt and writes back to
//! stdout, `GenericAgent` can drive it.

use std::path::Path;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, run_command};

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
/// use iter_core::{Agent, AgentRunContext, Prompt};
/// use iter_core::signal::SignalId;
/// use std::path::Path;
/// use tokio_util::sync::CancellationToken;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "cat".into()]);
/// let prompt = Prompt::from("hello");
/// let ctx = AgentRunContext::new(
///     Path::new("."),
///     &prompt,
///     CancellationToken::new(),
///     SignalId::new(),
/// );
/// let report = agent.run(ctx).await?;
/// assert!(report.exit_status.is_success());
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
        let (program, rest) = self.command.split_first().ok_or(AgentError::EmptyCommand)?;
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

impl Agent for GenericAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let command = self.build_command(ctx.workspace_path, ctx.prompt)?;
        let delivery = if self.stdin_prompt {
            PromptDelivery::Stdin(ctx.prompt.as_str())
        } else {
            PromptDelivery::Inline
        };
        run_command(command, delivery, ctx.cancel, ctx.stdio_sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::ctx;

    #[tokio::test]
    async fn captures_stdout_on_success() {
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "echo hello".into()]);
        let prompt = Prompt::from("ignored");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("hello"), "got {out:?}");
        assert!(report.turn_count.is_none());
    }

    #[tokio::test]
    async fn non_zero_exit_reported_as_failure() {
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "exit 7".into()]);
        let prompt = Prompt::from("ignored");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Failure(7));
    }

    #[tokio::test]
    async fn stdin_prompt_is_piped_to_child() {
        // `cat` with no args copies stdin to stdout; assert we see the prompt.
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "cat".into()]);
        let prompt = Prompt::from("from-stdin");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert!(
            report
                .last_output
                .expect("last_output")
                .contains("from-stdin")
        );
    }

    #[tokio::test]
    async fn inline_prompt_is_appended_as_arg() {
        // `sh -c 'printf %s "$0"' -- <prompt>`
        let agent = GenericAgent::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$0\"".into(),
            "placeholder".into(),
        ])
        .with_stdin_prompt(false);
        // The `placeholder` arg is the 4th element of `command`; the runtime
        // will also append the prompt. We assert the prompt *appears* in
        // stdout, since `$0` is `placeholder` — the assertion is that the
        // arg was appended without error. Swap to env-based echo to observe
        // the appended arg directly.
        let prompt = Prompt::from("appended");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        // The appended prompt is the *5th* arg; $0 is still "placeholder".
        // Use a follow-up agent that explicitly echoes `$1`.
        assert_eq!(report.exit_status, ExitStatus::Success);

        let agent = GenericAgent::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$1\"".into(),
            "placeholder".into(),
        ])
        .with_stdin_prompt(false);
        let prompt = Prompt::from("appended-arg");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "appended-arg");
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
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value");
    }

    #[tokio::test]
    async fn empty_command_errors() {
        let agent = GenericAgent::new(Vec::new());
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("must fail");
        assert!(matches!(err, AgentError::EmptyCommand));
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
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "alpha beta");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_termination_is_reported() {
        // Child kills itself with SIGKILL (signal 9). On Unix, the parent
        // should see `ExitStatus::Signal(9)`.
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "kill -KILL $$".into()]);
        let prompt = Prompt::from("ignored");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Signal(9));
    }

    #[tokio::test]
    async fn working_directory_is_applied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let agent = GenericAgent::new(vec!["sh".into(), "-c".into(), "pwd".into()]);
        let prompt = Prompt::from("ignored");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");
        // Resolve the canonical path to avoid symlink mismatches on macOS.
        let canonical = tmp.path().canonicalize().expect("canonicalize");
        assert!(
            out.contains(canonical.to_string_lossy().as_ref()),
            "expected {canonical:?} in {out:?}"
        );
    }
}
