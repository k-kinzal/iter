//! [`AntigravityAgent`] — Google Antigravity CLI (`agy`) integration.
//!
//! Antigravity CLI is Google's successor to Gemini CLI, announced at
//! I/O 2026. The binary, flag surface, and hook protocol all differ
//! from the Gemini CLI driver (`drivers/gemini/`).
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   agy -p <prompt> [--conversation <id>] [extra-args...]
//!   ```
//!
//!   The prompt is delivered as the value of `-p`. The child's stdin is
//!   closed immediately and stdout is captured into
//!   [`AgentReport::last_output`](crate::AgentReport).
//!
//! * [`AgentMode::Interactive`] — launches `agy` as a live TUI:
//!
//!   ```text
//!   agy [--conversation <id>] <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until the Antigravity hook JSON
//!   schema stabilizes. The agent exits after the TUI session and
//!   iter captures the exit status only; `last_output` is `None`.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process. In non-tty environments use [`AgentMode::Print`].
//!
//! # Session persistence
//!
//! Unlike Gemini CLI, Antigravity has built-in session persistence via
//! `--conversation <id>`. When [`AntigravitySettings::conversation_id`]
//! is set, iter passes `--conversation <id>` on every invocation so the
//! agent resumes the same session. When unset, each iteration starts a
//! fresh conversation.
//!
//! # Construction
//!
//! [`AntigravityAgent`] exposes no defaults. Every field on
//! [`AntigravitySettings`] is required because the value is a
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

/// Fully-specified configuration for [`AntigravityAgent`].
///
/// Every field is required; there is no `Default` impl.
#[derive(Debug, Clone)]
pub struct AntigravitySettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Print vs. interactive mode.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    /// Empty is allowed.
    pub args: Vec<String>,
    /// Optional conversation ID for session persistence. When set,
    /// `--conversation <id>` is passed to the binary.
    pub conversation_id: Option<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
    /// Build a fully-specified Antigravity agent. Every knob must be
    /// decided by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: AntigravitySettings) -> Self {
        let AntigravitySettings {
            command,
            mode,
            args,
            conversation_id,
            env,
        } = settings;
        Self {
            command,
            mode,
            args,
            conversation_id,
            env,
        }
    }

    fn append_conversation_flag(&self, cmd: &mut Command) {
        if let Some(ref id) = self.conversation_id {
            cmd.arg("--conversation").arg(id);
        }
    }

    fn build_print_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("-p").arg(prompt.as_str());
        self.append_conversation_flag(&mut cmd);
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        self.append_conversation_flag(&mut cmd);
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for AntigravityAgent {
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

impl AntigravityAgent {
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

    fn settings(command: impl Into<String>, mode: AgentMode) -> AntigravitySettings {
        AntigravitySettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            conversation_id: None,
            env: Vec::new(),
        }
    }

    #[tokio::test]
    async fn emits_dash_p_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = AntigravityAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-agy");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("-p"), "got {out:?}");
        assert!(out.contains("hello-agy"), "got {out:?}");
        let dash_pos = out.find("-p").expect("-p");
        let prompt_pos = out.find("hello-agy").expect("prompt");
        assert!(dash_pos < prompt_pos, "got {out:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$AGY_TEST_ENV_VAR\"");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("AGY_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = AntigravityAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value");
    }

    #[tokio::test]
    async fn conversation_id_adds_flag_when_set() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.conversation_id = Some("test-session-42".into());
        let agent = AntigravityAgent::new(s);
        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(out.contains("--conversation"), "got {out:?}");
        assert!(out.contains("test-session-42"), "got {out:?}");
    }

    #[tokio::test]
    async fn conversation_id_absent_produces_no_flag() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = AntigravityAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(!out.contains("--conversation"), "got {out:?}");
    }

    #[tokio::test]
    async fn conversation_id_precedes_extra_args_in_print_mode() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.conversation_id = Some("sess-1".into());
        s.args = vec!["--print-timeout".into(), "600".into()];
        let agent = AntigravityAgent::new(s);
        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        let conv_pos = out.find("--conversation").expect("--conversation");
        let extra_pos = out.find("--print-timeout").expect("--print-timeout");
        assert!(conv_pos < extra_pos, "got {out:?}");
    }

    #[tokio::test]
    async fn interactive_mode_passes_prompt_as_positional() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let argv_file = tmp.path().join("argv.txt");
        let script = format!("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > {argv_file:?}");
        let (_guard, bin) = fake_binary_script(&script);
        let agent = AntigravityAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("interactive-prompt");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        assert!(report.last_output.is_none());
        let argv_content = std::fs::read_to_string(&argv_file).expect("read argv");
        assert!(
            argv_content.contains("interactive-prompt"),
            "got {argv_content:?}",
        );
    }
}
