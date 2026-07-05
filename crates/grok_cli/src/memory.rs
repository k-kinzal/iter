//! `grok memory` — manage saved agent memory.
//!
//! `grok memory <COMMAND>` currently exposes a single leaf, `clear`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};
use crate::options::GlobalOptions;

/// A `grok memory` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemorySubcommand {
    /// `clear` — delete saved memory.
    Clear {
        /// `--workspace`: clear only the current workspace's memory.
        workspace: bool,
        /// `--global`: clear only global memory.
        global: bool,
        /// `--all`: clear both workspace and global memory.
        all: bool,
        /// `-y, --yes`: skip the confirmation prompt.
        yes: bool,
    },
}

impl MemorySubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Clear {
                workspace,
                global,
                all,
                yes,
            } => {
                args.push("clear".into());
                push_flag(args, *workspace, "--workspace");
                push_flag(args, *global, "--global");
                push_flag(args, *all, "--all");
                push_flag(args, *yes, "--yes");
            }
        }
    }
}

/// `grok memory [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `memory` subcommand.
    pub command: MemorySubcommand,
}

impl MemoryCommand {
    /// Build a `memory` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: MemorySubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
        }
    }
}

impl ToArgs for MemoryCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("memory".into());
        self.global.render(args);
        self.command.render(args);
    }
}
