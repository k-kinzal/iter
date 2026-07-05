//! `gemini hooks` — manage Gemini CLI hooks (alias `hook`).
//!
//! Gemini 0.41.2 exposes a single leaf, `hooks migrate`, which imports hooks
//! from Claude Code into Gemini CLI via its `--from-claude` flag.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};

/// `gemini hooks [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HooksCommand {
    /// `-d, --debug`.
    pub debug: bool,
    /// The hooks subcommand.
    pub command: HooksSubcommand,
}

impl HooksCommand {
    /// Wrap a hooks subcommand with default options.
    #[must_use]
    pub fn new(command: HooksSubcommand) -> Self {
        Self {
            debug: false,
            command,
        }
    }
}

impl ToArgs for HooksCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("hooks".into());
        push_flag(args, self.debug, "--debug");
        self.command.render(args);
    }
}

/// A `gemini hooks` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HooksSubcommand {
    /// `hooks migrate`: migrate hooks from Claude Code to Gemini CLI.
    Migrate {
        /// `--from-claude`: migrate from Claude Code hooks.
        from_claude: bool,
    },
}

impl HooksSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Migrate { from_claude } => {
                args.push("migrate".into());
                push_flag(args, *from_claude, "--from-claude");
            }
        }
    }
}
