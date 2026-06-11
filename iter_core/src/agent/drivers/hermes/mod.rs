//! [`HermesAgent`] — Nous Research Hermes Agent (`hermes`) integration.
//!
//! Hermes Agent is an open-source AI coding agent by Nous Research
//! (MIT license). It is Python-based with persistent memory, a rich
//! hook system, and multiple execution backends.
//!
//! The CLI-shaped argv construction and output/exit classification live at
//! the **Command level** in [`command`]; this module is the
//! **Driver/Adapter** that projects [`HermesResult`](command::HermesResult) /
//! [`HermesError`](command::HermesError) onto iter's domain
//! [`AgentRun`](crate::AgentRun) / [`AgentError`].
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   hermes -z <prompt> [extra-args...]
//!   ```
//!
//!   The `-z` flag is the scripted mode that suppresses banners,
//!   spinners, and cosmetic output. The prompt is delivered as the
//!   value of `-z` (inline argv, not stdin). There is **no JSON mode** in
//!   `-z`: stdout is the final assistant text only and stderr is
//!   `/dev/null`'d during the run, so classification is exit-code + text
//!   scan. See [`command`] for the full contract.
//!
//! * [`AgentMode::Interactive`] — launches `hermes` as a modal TUI:
//!
//!   ```text
//!   hermes --tui <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until the basic driver is proven (see the
//!   `hook` submodule). The agent exits after the TUI session and iter
//!   classifies the exit status only: a clean exit is a run, anything else
//!   is a failure. No session id is surfaced.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process. In non-tty environments use [`AgentMode::Print`].
//!
//! # Tool approval
//!
//! Hermes prompts for approval before executing tools. In
//! non-TTY environments (iter's use case), `--yolo` must be passed
//! via `args` to bypass prompts. iter does not hardcode this — the
//! operator decides via `args`.
//!
//! # Session persistence
//!
//! Hermes stores sessions in its own `SQLite` database and addresses
//! them by ID string. Operators can pass `--resume <id>` via `args`
//! to resume a specific session. iter does not manage Hermes sessions
//! directly, and `-z` mode surfaces no machine-readable session id, so
//! [`AgentRun::session_id`] is always `None`.
//!
//! # Construction
//!
//! [`HermesAgent`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentRun, AgentRunContext, Prompt};
use async_trait::async_trait;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod command;
pub(crate) mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize, spawn_capture,
};
use command::{HermesCommand, HermesError};

impl From<HermesError> for AgentError {
    /// Adapter projection: collapse Hermes' CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`HermesError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]. Exit `1`
    /// (uncaught) and exit `2` (bad args) both mean the agent never ran a
    /// turn, so they project onto [`AgentError::Launch`]; any other abnormal
    /// exit ([`HermesError::Failed`] — a non-`1`/`2` code or an indeterminate
    /// status) is a generic ran-but-failed and projects onto
    /// [`AgentError::Failed`] carrying the code when one exists; a signal
    /// becomes [`AgentError::TerminatedBySignal`].
    fn from(err: HermesError) -> Self {
        match err {
            HermesError::TokenLimit(detail) => Self::TokenLimit(detail),
            HermesError::Uncaught(detail) => Self::Launch(format!("hermes: {detail}")),
            HermesError::BadArgs => {
                Self::Launch("hermes rejected the invocation (bad arguments)".to_owned())
            }
            HermesError::Failed { exit_code, detail } => Self::Failed {
                code: exit_code,
                message: format!("hermes: {detail}"),
            },
            HermesError::Signal(sig) => Self::TerminatedBySignal(sig),
        }
    }
}

/// Hermes CLI agent configuration.
#[derive(Debug, Clone)]
pub struct HermesAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl HermesAgent {
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("--tui");
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

#[async_trait]
impl Agent for HermesAgent {
    fn name(&self) -> &'static str {
        "hermes"
    }

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            sandbox_command_prefix,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = HermesCommand {
                    program: &self.command,
                    prompt: prompt.as_str(),
                    args: &self.args,
                }
                .build(workspace_path);
                apply_user_env(&mut command, &self.env);
                // OTel trace-context / resource-attribute injection is
                // deliberately omitted: Hermes' consumption of `TRACEPARENT` /
                // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so iter does not
                // make its traces *look* correlated without confirming the
                // agent participates.
                //
                // The prompt is already embedded in argv (`-z <prompt>`), so
                // nothing is written to stdin.
                let output = spawn_capture(
                    command,
                    PromptDelivery::Inline,
                    cancel,
                    stdio_sink,
                    sandbox_command_prefix,
                )
                .await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<HermesError>` above.
                let result = command::interpret(&output)?;
                Ok(AgentRun {
                    session_id: result.session_id,
                })
            }
            AgentMode::Interactive => {
                self.run_interactive(workspace_path, prompt, cancel, sandbox_command_prefix)
                    .await
            }
        }
    }
}

impl HermesAgent {
    /// Drive `hermes` as a TUI session. Hook-based output capture is pending
    /// (see the `hook` submodule), so there is no bundle to install or
    /// finalize yet — a no-op finalize keeps the run-then-finalize skeleton in
    /// [`drive_interactive_with_finalize`] so wiring up a real
    /// `HookBundle::finalize()` later is a one-line change.
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        sandbox_prefix: &[OsString],
    ) -> Result<AgentRun, AgentError> {
        let mut command = self.build_interactive_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        // Interactive mode has no machine-readable output: the only signal is
        // the child's exit. A clean exit is a run; anything else is a failure.
        let exit =
            drive_interactive_with_finalize(command, cancel, sandbox_prefix, async { Ok(()) })
                .await?;
        if let Some(err) = exit.into_failure() {
            return Err(err);
        }
        Ok(AgentRun::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{ctx, ctx_capturing, fake_binary_script};

    fn hermes_agent(command: impl Into<String>, mode: AgentMode) -> HermesAgent {
        HermesAgent {
            command: command.into(),
            mode,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `hermes -z` binary: echoes each argv arg (one per line) to
    /// *stderr* so a [`CaptureSink`] can observe them, then prints a final
    /// response line to stdout so the Command parses an `Ok`.
    const FAKE_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf 'final response\n'"#;

    #[tokio::test]
    async fn print_mode_emits_dash_z_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("hello-hermes");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert_eq!(args.first(), Some(&"-z"), "got {args:?}");
        assert_eq!(args.get(1), Some(&"hello-hermes"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) =
            fake_binary_script("printf 'ENV=%s\\n' \"$HERMES_TEST_ENV_VAR\" 1>&2\nprintf 'ok\\n'");
        let mut s = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("HERMES_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn print_mode_extra_args_appended_after_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let mut s = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--yolo".into(), "--max-turns".into(), "30".into()];
        let agent = s;
        let prompt = Prompt::from("go");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        let prompt_pos = args.iter().position(|a| *a == "go").expect("prompt");
        let extra_pos = args.iter().position(|a| *a == "--yolo").expect("--yolo");
        assert!(prompt_pos < extra_pos, "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_nonzero_exit_maps_to_launch() {
        // Exit 1 with a traceback on stderr → uncaught → Launch.
        let (_guard, bin) = fake_binary_script("printf 'Traceback: boom\\n' 1>&2\nexit 1");
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("nonzero exit is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_exit_two_maps_to_launch() {
        let (_guard, bin) = fake_binary_script("exit 2");
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("bad args is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_token_limit_is_detected() {
        let (_guard, bin) = fake_binary_script("printf 'Error: context window exceeded\\n'");
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn interactive_mode_passes_tui_and_prompt_as_positional() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let argv_file = tmp.path().join("argv.txt");
        let script = format!("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > {argv_file:?}");
        let (_guard, bin) = fake_binary_script(&script);
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("interactive-prompt");
        let run = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let argv_content = std::fs::read_to_string(&argv_file).expect("read argv");
        let argv: Vec<&str> = argv_content.lines().collect();
        let tui_pos = argv.iter().position(|a| *a == "--tui").expect("--tui");
        let prompt_pos = argv
            .iter()
            .position(|a| *a == "interactive-prompt")
            .expect("prompt");
        assert!(tui_pos < prompt_pos, "got {argv:?}");
    }

    #[tokio::test]
    async fn interactive_mode_nonzero_exit_maps_to_failed() {
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = tempfile::TempDir::new().expect("tmp");
        let agent = hermes_agent(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }
}
