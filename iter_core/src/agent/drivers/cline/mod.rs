//! [`ClineAgent`] — Cline CLI integration.
//!
//! Cline is process-restart based: each invocation runs the agent to
//! completion with no hook plumbing.
//!
//! # Assumed CLI shape
//!
//! ```text
//! cline --oneshot [args...]
//! ```
//!
//! with the prompt on stdin. `--oneshot` runs a single turn and exits.
//!
//! # Assumptions to verify later
//!
//! - The flag is `--oneshot`. Some builds use `--single-turn` or `run`.
//! - Prompts are read from stdin.
//!
//! # Construction
//!
//! [`ClineAgent`] exposes no defaults. Every field on [`ClineSettings`]
//! is required because the value is a project-shaped decision iter
//! cannot honestly pick on the operator's behalf.

use std::path::Path;

use crate::{Agent, AgentReport, AgentRunContext};
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, run_command};

/// Fully-specified configuration for [`ClineAgent`].
#[derive(Debug, Clone)]
pub struct ClineSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Additional arguments appended after the built-in `--oneshot` flag.
    /// Empty is allowed.
    pub args: Vec<String>,
}

/// Cline CLI agent configuration.
#[derive(Debug, Clone)]
pub struct ClineAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the built-in `--oneshot` flag.
    pub args: Vec<String>,
}

impl ClineAgent {
    /// Build a fully-specified Cline agent.
    #[must_use]
    pub fn new(settings: ClineSettings) -> Self {
        let ClineSettings { command, args } = settings;
        Self { command, args }
    }

    fn build_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("--oneshot");
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for ClineAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let command = self.build_command(ctx.workspace_path);
        run_command(
            command,
            PromptDelivery::Stdin(ctx.prompt.as_str()),
            ctx.cancel,
            ctx.stdio_sink,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use crate::{ExitStatus, Prompt};

    #[tokio::test]
    async fn passes_oneshot_flag_and_stdin_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'; cat",
        );
        let agent = ClineAgent::new(ClineSettings {
            command: bin.to_string_lossy().into_owned(),
            args: Vec::new(),
        });
        let prompt = Prompt::from("hello-cline");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("args: --oneshot"), "got {out:?}");
        assert!(out.contains("hello-cline"), "got {out:?}");
    }
}
