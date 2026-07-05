//! `hermes sessions <command>` — the SQLite session store, and the `export`
//! JSONL surface.
//!
//! Every leaf but `export` is text-only. `sessions export <OUT>` writes the
//! store as JSONL — one JSON object per line (`-` writes to stdout) —
//! optionally filtered by `--source` or a single `--session-id`.
//! [`SessionExport`] parses that stream leniently, preserving each line's JSON
//! losslessly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_num, push_positional, push_positionals};

/// A `hermes sessions` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionsSubcommand {
    /// `list`: list recent sessions.
    List {
        /// `--source <SOURCE>`: filter by source (`cli`, `telegram`, …).
        source: Option<String>,
        /// `--limit <N>`: maximum sessions to show.
        limit: Option<u32>,
    },
    /// `export <OUTPUT>`: export sessions to a JSONL file (`-` for stdout).
    Export {
        /// The output JSONL path (`-` for stdout).
        output: PathBuf,
        /// `--source <SOURCE>`: filter by source.
        source: Option<String>,
        /// `--session-id <SESSION_ID>`: export a specific session.
        session_id: Option<String>,
    },
    /// `delete <SESSION_ID>`: delete a specific session.
    Delete {
        /// The session to delete.
        session_id: String,
        /// `--yes`: skip confirmation.
        yes: bool,
    },
    /// `prune`: delete old sessions.
    Prune {
        /// `--older-than <N>`: delete sessions older than N days (default 90).
        older_than: Option<u32>,
        /// `--source <SOURCE>`: only prune sessions from this source.
        source: Option<String>,
        /// `--yes`: skip confirmation.
        yes: bool,
    },
    /// `optimize`: reclaim disk space (merge FTS5 segments + VACUUM).
    Optimize,
    /// `repair`: repair a malformed `state.db` schema.
    Repair {
        /// `--check-only`: only report whether the database opens cleanly.
        check_only: bool,
        /// `--no-backup`: skip the timestamped backup copy.
        no_backup: bool,
    },
    /// `stats`: show session store statistics.
    Stats,
    /// `rename <SESSION_ID> <TITLE...>`: set or change a session's title.
    Rename {
        /// The session to rename.
        session_id: String,
        /// The new title, as one or more words.
        title: Vec<String>,
    },
    /// `browse`: interactive session picker.
    Browse,
}

impl SessionsSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { source, limit } => {
                args.push("list".into());
                push_opt(args, "--source", source.as_deref());
                push_opt_num(args, "--limit", *limit);
            }
            Self::Export {
                output,
                source,
                session_id,
            } => {
                args.push("export".into());
                push_opt(args, "--source", source.as_deref());
                push_opt(args, "--session-id", session_id.as_deref());
                push_positional(args, output);
            }
            Self::Delete { session_id, yes } => {
                args.push("delete".into());
                push_flag(args, *yes, "--yes");
                push_positional(args, session_id);
            }
            Self::Prune {
                older_than,
                source,
                yes,
            } => {
                args.push("prune".into());
                push_opt_num(args, "--older-than", *older_than);
                push_opt(args, "--source", source.as_deref());
                push_flag(args, *yes, "--yes");
            }
            Self::Optimize => args.push("optimize".into()),
            Self::Repair {
                check_only,
                no_backup,
            } => {
                args.push("repair".into());
                push_flag(args, *check_only, "--check-only");
                push_flag(args, *no_backup, "--no-backup");
            }
            Self::Stats => args.push("stats".into()),
            Self::Rename { session_id, title } => {
                args.push("rename".into());
                push_positional(args, session_id);
                push_positionals(args, title);
            }
            Self::Browse => args.push("browse".into()),
        }
    }
}

/// `hermes sessions <command>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsCommand {
    /// The leaf subcommand.
    pub subcommand: SessionsSubcommand,
}

impl SessionsCommand {
    /// Wrap a [`SessionsSubcommand`].
    #[must_use]
    pub fn new(subcommand: SessionsSubcommand) -> Self {
        Self { subcommand }
    }

    /// `hermes sessions list`.
    #[must_use]
    pub fn list() -> Self {
        Self::new(SessionsSubcommand::List {
            source: None,
            limit: None,
        })
    }

    /// `hermes sessions export <OUTPUT>`.
    #[must_use]
    pub fn export(output: impl Into<PathBuf>) -> Self {
        Self::new(SessionsSubcommand::Export {
            output: output.into(),
            source: None,
            session_id: None,
        })
    }

    /// `hermes sessions delete <SESSION_ID>`.
    #[must_use]
    pub fn delete(session_id: impl Into<String>) -> Self {
        Self::new(SessionsSubcommand::Delete {
            session_id: session_id.into(),
            yes: false,
        })
    }
}

impl ToArgs for SessionsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("sessions".into());
        self.subcommand.render(args);
    }
}

/// One line of `hermes sessions export` output, preserved losslessly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRecord {
    raw: Value,
}

impl ExportRecord {
    /// Wrap a raw JSON value as a record.
    #[must_use]
    pub fn from_value(raw: Value) -> Self {
        Self { raw }
    }

    /// Borrow the raw JSON value.
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.raw
    }

    /// Return the raw JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.raw
    }

    fn first_string(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .find_map(|key| self.raw.get(*key).and_then(Value::as_str))
    }

    /// The session id, from `session_id` / `id`.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.first_string(&["session_id", "id"])
    }

    /// The session title, from `title`.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.first_string(&["title"])
    }

    /// The session source, from `source`.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.first_string(&["source"])
    }
}

/// Parsed `hermes sessions export` JSONL output.
///
/// Non-JSON and non-object lines are skipped, mirroring how a JSONL export may
/// interleave incidental text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionExport {
    records: Vec<ExportRecord>,
}

impl SessionExport {
    /// Parse a `sessions export` JSONL stream leniently: every JSON-object line
    /// becomes an [`ExportRecord`], and any other line is ignored.
    #[must_use]
    pub fn parse(jsonl: &str) -> Self {
        let records = jsonl
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(Value::is_object)
            .map(ExportRecord::from_value)
            .collect();
        Self { records }
    }

    /// Read and parse a JSONL export file.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`std::io::Error`] when the file cannot be read.
    pub fn from_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let jsonl = std::fs::read_to_string(path)?;
        Ok(Self::parse(&jsonl))
    }

    /// The parsed records, in file order.
    #[must_use]
    pub fn records(&self) -> &[ExportRecord] {
        &self.records
    }

    /// The distinct session ids present across the export, in first-seen order.
    #[must_use]
    pub fn session_ids(&self) -> Vec<&str> {
        let mut seen = Vec::new();
        for id in self.records.iter().filter_map(ExportRecord::session_id) {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        seen
    }
}
