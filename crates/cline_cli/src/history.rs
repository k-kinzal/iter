//! `cline history` — inspect and manage recorded task sessions.
//!
//! The bare `cline history` invocation lists sessions; it is modeled here as
//! [`HistorySubcommand::List`] so every form flows through one enum.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_num, push_opt_path};

/// `cline history [COMMAND]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCommand {
    /// The history subcommand (`List` is the bare `cline history`).
    pub command: HistorySubcommand,
}

impl HistoryCommand {
    /// Wrap a history subcommand.
    #[must_use]
    pub fn new(command: HistorySubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for HistoryCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("history".into());
        self.command.render(args);
    }
}

/// A `cline history` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HistorySubcommand {
    /// The bare `cline history`: list recorded sessions.
    List {
        /// `--json`: output as JSON.
        json: bool,
        /// `--limit <count>` (Cline's default is `50`).
        limit: Option<u32>,
        /// `--page <number>`: page number for pagination.
        page: Option<u32>,
        /// `--config <dir>`: configuration directory.
        config: Option<PathBuf>,
    },
    /// `history delete`: delete a session.
    Delete {
        /// `--session-id <id>`: the session to delete.
        session_id: Option<String>,
    },
    /// `history update`: update session metadata.
    Update {
        /// `--metadata <json>`: metadata as a JSON object.
        metadata: Option<String>,
        /// `--prompt <text>`: replacement prompt text.
        prompt: Option<String>,
        /// `--session-id <id>`: the session to update.
        session_id: Option<String>,
        /// `--title <text>`: replacement title.
        title: Option<String>,
    },
    /// `history export <sessionId>`: export a session to a file.
    Export {
        /// Session ID positional.
        session_id: String,
        /// `-o, --output <path>`: output file path.
        output: Option<PathBuf>,
    },
}

impl HistorySubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List {
                json,
                limit,
                page,
                config,
            } => {
                push_flag(args, *json, "--json");
                push_opt_num(args, "--limit", *limit);
                push_opt_num(args, "--page", *page);
                push_opt_path(args, "--config", config.as_deref());
            }
            Self::Delete { session_id } => {
                args.push("delete".into());
                push_opt(args, "--session-id", session_id.as_deref());
            }
            Self::Update {
                metadata,
                prompt,
                session_id,
                title,
            } => {
                args.push("update".into());
                push_opt(args, "--metadata", metadata.as_deref());
                push_opt(args, "--prompt", prompt.as_deref());
                push_opt(args, "--session-id", session_id.as_deref());
                push_opt(args, "--title", title.as_deref());
            }
            Self::Export { session_id, output } => {
                args.push("export".into());
                push_opt_path(args, "--output", output.as_deref());
                args.push(session_id.into());
            }
        }
    }
}
