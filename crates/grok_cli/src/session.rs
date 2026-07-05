//! Session history: `grok sessions`, `grok export`, `grok import`,
//! `grok trace`.
//!
//! These are four sibling top-level commands (not one group), collected here
//! because they all read or move session history.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt_num, push_opt_path};
use crate::options::GlobalOptions;

/// A `grok sessions` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionsSubcommand {
    /// `list` — list recent sessions.
    List {
        /// `-n, --limit <LIMIT>`: number of sessions to show (grok defaults to
        /// 20 when omitted).
        limit: Option<u64>,
    },
    /// `search <QUERY>` — search sessions by content.
    Search {
        /// Search query.
        query: String,
        /// `-n, --limit <LIMIT>`: number of results to show (grok defaults to
        /// 20 when omitted).
        limit: Option<u64>,
    },
    /// `delete <ID>` — permanently delete a session from history.
    Delete {
        /// Session id to delete.
        id: String,
    },
}

impl SessionsSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { limit } => {
                args.push("list".into());
                push_opt_num(args, "--limit", *limit);
            }
            Self::Search { query, limit } => {
                args.push("search".into());
                push_opt_num(args, "--limit", *limit);
                args.push(query.into());
            }
            Self::Delete { id } => {
                args.push("delete".into());
                args.push(id.into());
            }
        }
    }
}

/// `grok sessions [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `sessions` subcommand.
    pub command: SessionsSubcommand,
}

impl SessionsCommand {
    /// Build a `sessions` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: SessionsSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
        }
    }
}

impl ToArgs for SessionsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("sessions".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// `grok export <SESSION_ID> [OUTPUT]` — export a session transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// Session id to export.
    pub session_id: String,
    /// Optional output path; grok writes to a default location when omitted.
    pub output: Option<PathBuf>,
    /// `-c, --clipboard`: copy the export to the clipboard.
    pub clipboard: bool,
}

impl ExportCommand {
    /// Export the session with the given id.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            global: GlobalOptions::default(),
            session_id: session_id.into(),
            output: None,
            clipboard: false,
        }
    }
}

impl ToArgs for ExportCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("export".into());
        self.global.render(args);
        push_flag(args, self.clipboard, "--clipboard");
        args.push((&self.session_id).into());
        if let Some(output) = &self.output {
            args.push(output.into());
        }
    }
}

/// `grok import [TARGETS]...` — import session transcripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// Import targets (paths or ids); may be empty.
    pub targets: Vec<String>,
    /// `--list`: list importable sessions instead of importing.
    pub list: bool,
    /// `--json`: emit machine-readable JSON output.
    pub json: bool,
}

impl ToArgs for ImportCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("import".into());
        self.global.render(args);
        push_flag(args, self.list, "--list");
        push_flag(args, self.json, "--json");
        for target in &self.targets {
            args.push(target.into());
        }
    }
}

/// `grok trace <SESSION_ID>` — render a session's execution trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// Session id to trace.
    pub session_id: String,
    /// `--local`: read the trace from local storage only.
    pub local: bool,
    /// `-o, --output <OUTPUT>`: write the trace to a file.
    pub output: Option<PathBuf>,
    /// `--json`: emit machine-readable JSON output.
    pub json: bool,
}

impl TraceCommand {
    /// Trace the session with the given id.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            global: GlobalOptions::default(),
            session_id: session_id.into(),
            local: false,
            output: None,
            json: false,
        }
    }
}

impl ToArgs for TraceCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("trace".into());
        self.global.render(args);
        push_flag(args, self.local, "--local");
        push_opt_path(args, "--output", self.output.as_deref());
        push_flag(args, self.json, "--json");
        args.push((&self.session_id).into());
    }
}
