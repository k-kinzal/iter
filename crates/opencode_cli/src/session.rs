//! `opencode session` — manage sessions.

use std::ffi::OsString;

use crate::args::{ToArgs, push_enum, push_opt_display};
use crate::options::GlobalOptions;
use crate::values::SessionFormat;

/// `opencode session <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The session subcommand.
    pub command: SessionSubcommand,
}

impl SessionCommand {
    /// Wrap a session subcommand with default global options.
    #[must_use]
    pub fn new(command: SessionSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for SessionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("session".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode session` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionSubcommand {
    /// `session list`: list sessions.
    List {
        /// `-n, --max-count <N>`: limit to the `N` most recent sessions.
        max_count: Option<u32>,
        /// `--format <table|json>`: the listing output format.
        format: Option<SessionFormat>,
    },
    /// `session delete <sessionID>`: delete a session.
    Delete {
        /// The session id to delete.
        session_id: String,
    },
}

impl SessionSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { max_count, format } => {
                args.push("list".into());
                push_opt_display(args, "--max-count", *max_count);
                push_enum(args, "--format", format.map(SessionFormat::as_str));
            }
            Self::Delete { session_id } => {
                args.push("delete".into());
                args.push(session_id.into());
            }
        }
    }
}
