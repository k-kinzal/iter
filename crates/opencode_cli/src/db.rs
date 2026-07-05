//! `opencode db` — database tools.

use std::ffi::OsString;

use crate::args::{ToArgs, push_enum};
use crate::options::GlobalOptions;
use crate::values::DbFormat;

/// `opencode db [COMMAND]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The db subcommand (the query form is the default).
    pub command: DbSubcommand,
}

impl DbCommand {
    /// Wrap a db subcommand with default global options.
    #[must_use]
    pub fn new(command: DbSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for DbCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("db".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode db` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DbSubcommand {
    /// `db [query]` (default): open an interactive sqlite3 shell, or run a
    /// query when one is supplied.
    Query {
        /// Optional SQL query to execute; `None` opens the interactive shell.
        query: Option<String>,
        /// `--format <json|tsv>` for the query result.
        format: Option<DbFormat>,
    },
    /// `db path`: print the database path.
    Path,
    /// `db migrate`: migrate JSON data to SQLite (merges with existing data).
    Migrate,
}

impl DbSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Query { query, format } => {
                // The query form is opencode's default command — no leaf token.
                push_enum(args, "--format", format.map(DbFormat::as_str));
                if let Some(query) = query {
                    args.push(query.into());
                }
            }
            Self::Path => args.push("path".into()),
            Self::Migrate => args.push("migrate".into()),
        }
    }
}
