//! [`CursorAgent`] — Cursor `cursor-agent` CLI integration.
//!
//! Cursor's CLI is process-restart based: it has no hook plumbing and runs
//! to completion on each invocation. This agent therefore has no
//! interactive/print mode distinction.
//!
//! # Assumed CLI shape
//!
//! ```text
//! cursor-agent --print [args...]
//! ```
//!
//! with the prompt written to stdin. The `--print` flag causes the binary
//! to emit a single response to stdout and exit.
//!
//! # Assumptions to verify later
//!
//! - The binary is named `cursor-agent` (not `cursor`).
//! - `--print` is the non-interactive flag.
//! - Prompts are read from stdin.
//!
//! # Construction
//!
//! [`CursorAgent`] exposes no defaults. Every field on
//! [`CursorSettings`] is required because the value is a project-shaped
//! decision iter cannot honestly pick on the operator's behalf.

use std::path::Path;

use crate::{Agent, AgentReport, AgentRunContext};
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, run_command};

/// Fully-specified configuration for [`CursorAgent`].
#[derive(Debug, Clone)]
pub struct CursorSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Additional arguments appended after the built-in `--print` flag.
    /// Empty is allowed.
    pub args: Vec<String>,
}

/// Cursor `cursor-agent` CLI agent configuration.
#[derive(Debug, Clone)]
pub struct CursorAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the built-in `--print` flag.
    pub args: Vec<String>,
}

impl CursorAgent {
    /// Build a fully-specified Cursor agent.
    #[must_use]
    pub fn new(settings: CursorSettings) -> Self {
        let CursorSettings { command, args } = settings;
        Self { command, args }
    }

    fn build_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("--print");
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for CursorAgent {
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
    async fn passes_print_flag_and_stdin_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'; cat",
        );
        let agent = CursorAgent::new(CursorSettings {
            command: bin.to_string_lossy().into_owned(),
            args: Vec::new(),
        });
        let prompt = Prompt::from("hello-cursor");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("args: --print"), "got {out:?}");
        assert!(out.contains("hello-cursor"), "got {out:?}");
    }
}
