//! `agy [OPTIONS] [PROMPT]` — the root run in its three modes.
//!
//! With no subcommand, `agy` runs the model. The prompt is delivered one of
//! three ways, selected by [`RunMode`]:
//!
//! * [`RunMode::Print`] — `--print <PROMPT>`: run once, non-interactively, and
//!   print the response.
//! * [`RunMode::PromptInteractive`] — `--prompt-interactive <PROMPT>`: seed an
//!   interactive session with an initial prompt, then continue it.
//! * [`RunMode::Interactive`] — the default: launch the interactive TUI. An
//!   optional positional prompt seeds the first turn.
//!
//! `agy`'s root parser is Go's standard `flag` package, which stops treating
//! arguments as flags at the first bare positional. All options are therefore
//! rendered *before* the positional prompt so `--conversation` and friends are
//! never swallowed as positionals.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_path, push_pair, push_paths};
use crate::values::GoDuration;

/// How the prompt is delivered to the root `agy` run.
///
/// Each mode carries its own prompt operand so a required prompt can never be
/// missing: `--print` and `--prompt-interactive` demand a prompt, while the
/// interactive TUI's seed prompt is genuinely optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunMode {
    /// `-p` / `--print` (`--prompt`): run a single prompt non-interactively and
    /// print the response. The prompt is required.
    Print(String),
    /// `-i` / `--prompt-interactive`: run an initial prompt interactively and
    /// continue the session. The prompt is required.
    PromptInteractive(String),
    /// Default: launch the interactive TUI, optionally seeded by a positional
    /// prompt.
    Interactive(Option<String>),
}

impl Default for RunMode {
    /// The default run mode is the interactive TUI with no seed prompt.
    fn default() -> Self {
        Self::Interactive(None)
    }
}

/// Root-run options shared across every [`RunMode`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `--add-dir <DIR>` (repeatable): add a directory to the workspace.
    pub add_dir: Vec<PathBuf>,
    /// `-c` / `--continue`: continue the most recent conversation.
    pub continue_conversation: bool,
    /// `--conversation <ID>`: resume a previous conversation by ID.
    pub conversation: Option<String>,
    /// `--dangerously-skip-permissions`: auto-approve all tool permissions.
    pub dangerously_skip_permissions: bool,
    /// `--log-file <PATH>`: override the CLI log file path.
    pub log_file: Option<PathBuf>,
    /// `--model <MODEL>`: model for this CLI session.
    pub model: Option<String>,
    /// `--new-project`: create a new project for this session.
    pub new_project: bool,
    /// `--print-timeout <DURATION>`: timeout for print-mode wait (default 5m).
    pub print_timeout: Option<GoDuration>,
    /// `--project <ID>`: project ID for this CLI session.
    pub project: Option<String>,
    /// `--sandbox`: run in a sandbox with terminal restrictions enabled.
    pub sandbox: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_paths(args, "--add-dir", &self.add_dir);
        push_flag(args, self.continue_conversation, "--continue");
        push_opt(args, "--conversation", self.conversation.as_deref());
        push_flag(
            args,
            self.dangerously_skip_permissions,
            "--dangerously-skip-permissions",
        );
        push_opt_path(args, "--log-file", self.log_file.as_deref());
        push_opt(args, "--model", self.model.as_deref());
        push_flag(args, self.new_project, "--new-project");
        if let Some(timeout) = &self.print_timeout {
            push_pair(args, "--print-timeout", timeout.render());
        }
        push_opt(args, "--project", self.project.as_deref());
        push_flag(args, self.sandbox, "--sandbox");
    }
}

/// `agy [OPTIONS] [PROMPT]` — the root run.
///
/// The prompt operand lives inside [`RunMode`], so the required prompt for
/// `--print` / `--prompt-interactive` can never be dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// How the prompt is delivered, carrying the prompt operand.
    pub mode: RunMode,
    /// Root options rendered before the prompt.
    pub options: RunOptions,
}

impl RunCommand {
    /// `--print <PROMPT>`: run a single prompt non-interactively.
    #[must_use]
    pub fn print(prompt: impl Into<String>) -> Self {
        Self {
            mode: RunMode::Print(prompt.into()),
            ..Self::default()
        }
    }

    /// `--prompt-interactive <PROMPT>`: seed an interactive session.
    #[must_use]
    pub fn prompt_interactive(prompt: impl Into<String>) -> Self {
        Self {
            mode: RunMode::PromptInteractive(prompt.into()),
            ..Self::default()
        }
    }

    /// Launch the interactive TUI with no seed prompt.
    #[must_use]
    pub fn interactive() -> Self {
        Self {
            mode: RunMode::Interactive(None),
            ..Self::default()
        }
    }

    /// Launch the interactive TUI seeded by a positional prompt.
    #[must_use]
    pub fn interactive_prompt(prompt: impl Into<String>) -> Self {
        Self {
            mode: RunMode::Interactive(Some(prompt.into())),
            ..Self::default()
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        // Options first: Go's `flag` parser stops at the first positional.
        self.options.render(args);
        match &self.mode {
            RunMode::Print(prompt) => push_pair(args, "--print", prompt),
            RunMode::PromptInteractive(prompt) => {
                push_pair(args, "--prompt-interactive", prompt);
            }
            RunMode::Interactive(prompt) => {
                if let Some(prompt) = prompt {
                    args.push(prompt.into());
                }
            }
        }
    }
}
