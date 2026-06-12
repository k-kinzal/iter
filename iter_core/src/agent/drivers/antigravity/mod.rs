//! [`AntigravityAgent`] — Google Antigravity CLI (`agy`) integration.
//!
//! Antigravity CLI is Google's successor to Gemini CLI, announced at
//! I/O 2026. The binary, flag surface, and hook protocol all differ
//! from the Gemini CLI driver (`drivers/gemini/`).
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Spawns:
//!
//!   ```text
//!   agy -p <prompt> [--conversation <id>] [extra-args...]
//!   ```
//!
//!   The prompt is delivered inline as the value of `-p`. The child's stdin
//!   is closed immediately and its complete stdout/stderr is captured so the
//!   Command can classify the run. There is **no JSON mode** — see
//!   [`command`] for the text-marker contract.
//!
//! * [`AgentMode::Interactive`] — launches `agy` as a live TUI:
//!
//!   ```text
//!   agy [--conversation <id>] <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until the Antigravity hook JSON schema
//!   stabilizes (see [`hook`]); interactive mode therefore installs no hook
//!   bundle. The agent exits after the TUI session and iter captures the
//!   exit status only. Interactive mode inherits stdin/stdout/stderr from
//!   the parent process; in non-tty environments use [`AgentMode::Headless`].
//!
//! # Session persistence
//!
//! Unlike Gemini CLI, Antigravity has built-in session persistence via
//! `--conversation <id>`. When [`AntigravityAgent::conversation_id`]
//! is set, iter passes `--conversation <id>` on every invocation so the
//! agent resumes the same session. When unset, each iteration starts a
//! fresh conversation. `agy` does not echo the conversation id back in a
//! machine-readable form, so [`AgentRun::session_id`] is always `None`.
//!
//! # Construction
//!
//! [`AntigravityAgent`] exposes no defaults. Every field is required because
//! the value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentInvocation, AgentRun, Prompt};
use async_trait::async_trait;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod command;
pub(crate) mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{PromptDelivery, apply_user_env, drive_interactive, spawn_capture};
use command::{AntigravityCommand, AntigravityError};

impl From<AntigravityError> for AgentError {
    /// Adapter projection: collapse Antigravity's CLI-shaped error hierarchy
    /// onto iter's minimal domain error. Auth and TTY failures mean the agent
    /// never ran, so they become [`AgentError::Launch`]; only
    /// [`AntigravityError::TokenLimit`] is router-relevant and preserved.
    fn from(err: AntigravityError) -> Self {
        match err {
            AntigravityError::Auth => {
                Self::Launch("antigravity requires authentication".to_owned())
            }
            AntigravityError::LaunchTty => {
                Self::Launch("antigravity could not open a TTY".to_owned())
            }
            AntigravityError::TokenLimit(detail) => Self::TokenLimit(detail),
            AntigravityError::Launch(code) => {
                Self::Launch(format!("antigravity failed to launch (exit code {code})"))
            }
            AntigravityError::Signal(sig) => Self::TerminatedBySignal(sig),
            AntigravityError::Failed { exit_code } => Self::Failed {
                code: exit_code,
                message: "antigravity exited with a failure".to_owned(),
            },
        }
    }
}

/// Antigravity CLI agent configuration.
#[derive(Debug, Clone)]
pub struct AntigravityAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    pub args: Vec<String>,
    /// Optional conversation ID for session persistence.
    pub conversation_id: Option<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl AntigravityAgent {
    /// Build the interactive-mode command. Passes the prompt as the first
    /// positional argument so `agy` seeds its initial user turn with it
    /// before dropping into the TUI. Extra args come afterward so users can
    /// still inject their own flags.
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        if let Some(ref id) = self.conversation_id {
            cmd.arg("--conversation").arg(id);
        }
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

#[async_trait]
impl Agent for AntigravityAgent {
    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn kind(&self) -> crate::agent::AgentKind {
        crate::agent::AgentKind::Antigravity
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
        match self.mode {
            AgentMode::Headless => {
                let mut command = AntigravityCommand {
                    program: &self.command,
                    prompt: prompt.as_str(),
                    conversation_id: self.conversation_id.as_deref(),
                    args: &self.args,
                }
                .build(workspace_path);
                apply_user_env(&mut command, &self.env);
                // Prompt is embedded inline as the value of `-p`; no stdin.
                let output = spawn_capture(
                    command,
                    PromptDelivery::Inline,
                    cancel,
                    stdio_sink,
                    sandbox_command_prefix,
                )
                .await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<AntigravityError>` above.
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

impl AntigravityAgent {
    /// Drive `agy` as a TUI session. Antigravity's hook protocol is not yet
    /// stable (see [`hook`]), so no hook bundle is installed — there is
    /// nothing to finalize, and the plain [`drive_interactive`] skeleton is
    /// used directly.
    ///
    /// Interactive mode has no machine-readable output: the only signal is
    /// the child's exit. A clean exit is a run; anything else is a failure.
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

        let exit = drive_interactive(command, &cancel, sandbox_prefix).await?;
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
    use std::path::Path;

    fn antigravity_agent(command: impl Into<String>, mode: AgentMode) -> AntigravityAgent {
        AntigravityAgent {
            command: command.into(),
            mode,
            args: Vec::new(),
            conversation_id: None,
            env: Vec::new(),
        }
    }

    /// Fake `agy` print binary: echoes each argv arg to *stderr* (so a
    /// `CaptureSink` can observe the argv) and prints a final line to stdout
    /// so the Command parses an `Ok`.
    const FAKE_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf 'final answer'"#;

    #[tokio::test]
    async fn emits_dash_p_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-agy");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"-p"), "got {args:?}");
        assert!(args.contains(&"hello-agy"), "got {args:?}");
        let dash_pos = args.iter().position(|a| *a == "-p").expect("-p");
        let prompt_pos = args.iter().position(|a| *a == "hello-agy").expect("prompt");
        assert!(dash_pos < prompt_pos, "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$AGY_TEST_ENV_VAR\"");
        let mut s = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        s.env = vec![("AGY_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert_eq!(sink.stdout().await, "env-value");
    }

    #[tokio::test]
    async fn conversation_id_adds_flag_when_set() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let mut s = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        s.conversation_id = Some("test-session-42".into());
        let agent = s;
        let prompt = Prompt::from("go");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        assert!(echoed.contains("--conversation"), "got {echoed:?}");
        assert!(echoed.contains("test-session-42"), "got {echoed:?}");
    }

    #[tokio::test]
    async fn conversation_id_absent_produces_no_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("go");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(
            !sink.stderr().await.contains("--conversation"),
            "unset conversation_id must not emit --conversation",
        );
    }

    #[tokio::test]
    async fn conversation_id_precedes_extra_args_in_print_mode() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let mut s = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        s.conversation_id = Some("sess-1".into());
        s.args = vec!["--print-timeout".into(), "600".into()];
        let agent = s;
        let prompt = Prompt::from("go");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let conv_pos = echoed.find("--conversation").expect("--conversation");
        let extra_pos = echoed.find("--print-timeout").expect("--print-timeout");
        assert!(conv_pos < extra_pos, "got {echoed:?}");
    }

    #[tokio::test]
    async fn auth_marker_maps_to_launch_error() {
        let (_guard, bin) = fake_binary_script("printf 'Authentication required\\n' 1>&2\nexit 0");
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("auth must error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn token_limit_marker_maps_to_token_limit() {
        let (_guard, bin) =
            fake_binary_script("printf 'Error: context window exceeded\\n'\nexit 0");
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("token limit must error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn nonzero_exit_maps_to_failed() {
        let (_guard, bin) = fake_binary_script("exit 1");
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("nonzero exit must error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn interactive_mode_passes_prompt_as_positional() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let argv_file = tmp.path().join("argv.txt");
        let script = format!("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > {argv_file:?}");
        let (_guard, bin) = fake_binary_script(&script);
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("interactive-prompt");
        let run = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let argv_content = std::fs::read_to_string(&argv_file).expect("read argv");
        assert!(
            argv_content.contains("interactive-prompt"),
            "got {argv_content:?}",
        );
    }

    #[tokio::test]
    async fn interactive_nonzero_exit_is_failed() {
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = tempfile::TempDir::new().expect("tmp");
        let agent = antigravity_agent(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect_err("nonzero exit must error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }
}
