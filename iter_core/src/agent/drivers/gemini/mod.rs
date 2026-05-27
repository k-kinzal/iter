//! [`GeminiAgent`] — Google Gemini CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   gemini -p <prompt> [extra-args...]
//!   ```
//!
//!   The prompt is delivered as the value of `-p`, matching the common
//!   `gemini -p 'explain foo'` invocation pattern. The child's stdin is
//!   closed immediately and stdout is captured into
//!   [`AgentReport::last_output`](crate::AgentReport).
//!
//! * [`AgentMode::Interactive`] — launches `gemini` as a live TUI with a
//!   project-local `AfterAgent` hook installed under `${cwd}/.gemini/`.
//!   The hook's sole purpose is to terminate the TUI session after the
//!   agent finishes its task — it runs any pre-existing user hooks,
//!   then sends SIGKILL to the Gemini CLI process.
//!
//!   The hook is a direct descendant of
//!   [`agent-loop/gemini-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to terminate the
//!   TUI session.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.gemini/`. iter never writes to `~/.gemini/` because
//!   doing so would silently affect every other Gemini CLI session on
//!   the machine. See the `hook` submodule for the
//!   filesystem layout.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process so `gemini`'s TUI renders correctly when iter is invoked
//!   from a terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Print`] instead.
//!
//! # Assumptions to verify later
//!
//! - The short flag `-p` is the correct way to pass a prompt
//!   non-interactively. Some Gemini CLI builds use `--prompt`.
//! - `gemini` (no subcommand) drops into the interactive TUI and accepts
//!   a positional prompt for the initial user turn.
//! - Output appears on stdout in print mode.
//!
//! Override via [`args`](GeminiAgent::args) or
//! [`command`](GeminiAgent::command).
//!
//! # Construction
//!
//! [`GeminiAgent`] exposes no defaults. Every field on [`GeminiSettings`]
//! is required because the value is a project-shaped decision iter
//! cannot honestly pick on the operator's behalf.

use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize, run_command,
};
use hook::HookBundle;

/// Fully-specified configuration for [`GeminiAgent`].
///
/// Every field is required; there is no `Default` impl.
#[derive(Debug, Clone)]
pub struct GeminiSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Print vs. interactive mode.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in `-p <prompt>` pair
    /// (print mode) or after the prompt positional (interactive mode).
    /// Empty is allowed.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

/// Gemini CLI agent configuration.
#[derive(Debug, Clone)]
pub struct GeminiAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in `-p <prompt>` pair
    /// (print mode) or after the prompt positional (interactive mode).
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl GeminiAgent {
    /// Build a fully-specified Gemini agent. Every knob must be decided
    /// by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: GeminiSettings) -> Self {
        let GeminiSettings {
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
        cmd.arg("-p").arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    /// Build the interactive-mode command. Passes the prompt as the
    /// first positional argument so `gemini` seeds its initial user
    /// turn with it before dropping into the TUI; extras come after so
    /// users can still inject their own flags.
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for GeminiAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            service_name,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = self.build_print_command(workspace_path, prompt);
                apply_user_env(&mut command, &self.env);
                run_command(command, PromptDelivery::Inline, cancel, stdio_sink).await
            }
            AgentMode::Interactive => {
                self.run_interactive(workspace_path, prompt, cancel, &service_name)
                    .await
            }
        }
    }
}

impl GeminiAgent {
    /// Drive `gemini` as a TUI session. Installs the project-local
    /// `AfterAgent` hook bundle before spawning and finalizes it after
    /// — even on error paths — so the user's original `settings.json`
    /// is always restored.
    ///
    /// The run-then-finalize skeleton lives in
    /// [`drive_interactive_with_finalize`]; this method only handles the
    /// Gemini-specific bits: bundle install, command construction, and
    /// stdio inheritance wiring.
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        service_name: &str,
    ) -> Result<AgentReport, AgentError> {
        let bundle = HookBundle::install(path, service_name).await?;

        let mut command = self.build_interactive_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        drive_interactive_with_finalize(command, cancel, bundle.finalize()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>, mode: AgentMode) -> GeminiSettings {
        GeminiSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    #[tokio::test]
    async fn emits_dash_p_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = GeminiAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-gemini");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("-p"), "got {out:?}");
        assert!(out.contains("hello-gemini"), "got {out:?}");
        let dash_pos = out.find("-p").expect("-p");
        let prompt_pos = out.find("hello-gemini").expect("prompt");
        assert!(dash_pos < prompt_pos, "got {out:?}");
    }

    /// Fake `gemini` binary for interactive mode. Invokes the installed
    /// `AfterAgent` hook. The hook drains stdin and SIGKILLs `$PPID`.
    const FAKE_GEMINI_SCRIPT: &str = r#"
set -euo pipefail
HOOK="$PWD/.gemini/hooks/gemini-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_settings() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_GEMINI_SCRIPT);

        let settings_path = tmp.path().join(".gemini/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap())
            .await
            .expect("mkdir .gemini");
        let user_settings = json!({ "user_owned": true });
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&user_settings).unwrap(),
        )
        .await
        .expect("write settings");

        let agent = GeminiAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));

        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");

        assert!(
            report.exit_status == ExitStatus::Success
                || matches!(report.exit_status, ExitStatus::Signal(_)),
            "expected success or signal, got {:?}",
            report.exit_status,
        );

        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_settings,
            "user settings.json must be restored after interactive run",
        );
        assert!(
            !tmp.path().join(".gemini/hooks").exists(),
            "hooks directory must be cleaned up",
        );
        assert!(
            !tmp.path().join(".gemini/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$GEMINI_TEST_ENV_VAR\"");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("GEMINI_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = GeminiAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value",);
    }
}
