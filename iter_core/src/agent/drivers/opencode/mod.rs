//! [`OpenCodeAgent`] — `OpenCode` CLI integration.
//!
//! # Assumed CLI shape
//!
//! ```text
//! opencode run [args...] <prompt>
//! ```
//!
//! The prompt is passed as the final positional argument.
//!
//! # Assumptions to verify later
//!
//! - The subcommand is `run`.
//! - Prompts are positional, not passed via a flag.
//!
//! # Construction
//!
//! [`OpenCodeAgent`] exposes no defaults. Every field on
//! [`OpenCodeSettings`] is required because the value is a project-shaped
//! decision iter cannot honestly pick on the operator's behalf.

use std::path::Path;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, apply_user_env, run_command};

/// Fully-specified configuration for [`OpenCodeAgent`].
#[derive(Debug, Clone)]
pub struct OpenCodeSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Additional arguments inserted between the `run` subcommand and the
    /// positional prompt. Empty is allowed.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

/// `OpenCode` CLI agent configuration.
#[derive(Debug, Clone)]
pub struct OpenCodeAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments inserted between the `run` subcommand and the
    /// positional prompt.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl OpenCodeAgent {
    /// Build a fully-specified `OpenCode` agent.
    #[must_use]
    pub fn new(settings: OpenCodeSettings) -> Self {
        let OpenCodeSettings { command, args, env } = settings;
        Self { command, args, env }
    }

    fn build_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("run");
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(prompt.as_str());
        apply_user_env(&mut cmd, &self.env);
        cmd
    }
}

impl Agent for OpenCodeAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let command = self.build_command(ctx.workspace_path, ctx.prompt);
        run_command(command, PromptDelivery::Inline, ctx.cancel, ctx.stdio_sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};

    #[tokio::test]
    async fn passes_run_subcommand_and_inline_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = OpenCodeAgent::new(OpenCodeSettings {
            command: bin.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: Vec::new(),
        });
        let prompt = Prompt::from("hello-opencode");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("args: run"), "got {out:?}");
        assert!(out.contains("hello-opencode"), "got {out:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$OPENCODE_TEST_ENV_VAR\"");
        let agent = OpenCodeAgent::new(OpenCodeSettings {
            command: bin.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: vec![("OPENCODE_TEST_ENV_VAR".into(), "env-value".into())],
        });
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value",);
    }
}
