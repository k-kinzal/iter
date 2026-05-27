//! [`HermesAgent`] — Nous Research Hermes Agent (`hermes`) integration.
//!
//! Hermes Agent is an open-source AI coding agent by Nous Research
//! (MIT license). It is Python-based with persistent memory, a rich
//! hook system, and multiple execution backends.
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
//!   value of `-z`. The child's stdin is closed immediately and stdout
//!   is captured into [`AgentReport::last_output`](crate::AgentReport).
//!
//! * [`AgentMode::Interactive`] — launches `hermes` as a modal TUI:
//!
//!   ```text
//!   hermes --tui <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until the basic driver is proven.
//!   The agent exits after the TUI session and iter captures the exit
//!   status only; `last_output` is `None`.
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
//! directly.
//!
//! # Construction
//!
//! [`HermesAgent`] exposes no defaults. Every field on
//! [`HermesSettings`] is required because the value is a
//! project-shaped decision iter cannot honestly pick on the operator's
//! behalf.

use std::path::Path;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub(crate) mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{PromptDelivery, apply_user_env, drive_interactive_child, run_command};

/// Fully-specified configuration for [`HermesAgent`].
///
/// Every field is required; there is no `Default` impl.
#[derive(Debug, Clone)]
pub struct HermesSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Print vs. interactive mode.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    /// Empty is allowed.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
    /// Build a fully-specified Hermes agent. Every knob must be
    /// decided by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: HermesSettings) -> Self {
        let HermesSettings {
            command,
            mode,
            args,
            env,
        } = settings;
        Self {
            command,
            mode,
            args,
            env,
        }
    }

    fn build_print_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("-z").arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

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

impl Agent for HermesAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = self.build_print_command(workspace_path, prompt);
                apply_user_env(&mut command, &self.env);
                run_command(command, PromptDelivery::Inline, cancel, stdio_sink).await
            }
            AgentMode::Interactive => self.run_interactive(workspace_path, prompt, cancel).await,
        }
    }
}

impl HermesAgent {
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
    ) -> Result<AgentReport, AgentError> {
        use std::process::Stdio;

        let mut command = self.build_interactive_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let exit_status = drive_interactive_child(command, &cancel).await?;
        Ok(AgentReport {
            exit_status,
            last_output: None,
            turn_count: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use std::path::Path;

    fn settings(command: impl Into<String>, mode: AgentMode) -> HermesSettings {
        HermesSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    #[tokio::test]
    async fn emits_dash_z_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = HermesAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-hermes");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("-z"), "got {out:?}");
        assert!(out.contains("hello-hermes"), "got {out:?}");
        let dash_pos = out.find("-z").expect("-z");
        let prompt_pos = out.find("hello-hermes").expect("prompt");
        assert!(dash_pos < prompt_pos, "got {out:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$HERMES_TEST_ENV_VAR\"");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("HERMES_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = HermesAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value");
    }

    #[tokio::test]
    async fn extra_args_appended_after_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--yolo".into(), "--max-turns".into(), "30".into()];
        let agent = HermesAgent::new(s);
        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        let prompt_pos = out.find("go").expect("prompt");
        let extra_pos = out.find("--yolo").expect("--yolo");
        assert!(prompt_pos < extra_pos, "got {out:?}");
    }

    #[tokio::test]
    async fn interactive_mode_passes_tui_and_prompt_as_positional() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let argv_file = tmp.path().join("argv.txt");
        let script = format!("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > {argv_file:?}");
        let (_guard, bin) = fake_binary_script(&script);
        let agent = HermesAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("interactive-prompt");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        assert!(report.last_output.is_none());
        let argv_content = std::fs::read_to_string(&argv_file).expect("read argv");
        assert!(
            argv_content.contains("--tui"),
            "got {argv_content:?}",
        );
        assert!(
            argv_content.contains("interactive-prompt"),
            "got {argv_content:?}",
        );
        let tui_pos = argv_content.find("--tui").expect("--tui");
        let prompt_pos = argv_content.find("interactive-prompt").expect("prompt");
        assert!(tui_pos < prompt_pos, "got {argv_content:?}");
    }
}
