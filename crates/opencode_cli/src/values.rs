//! Typed value enums for opencode's value-taking flags.

use std::ffi::OsString;

use crate::args::{push_flag, push_pair};

/// `--format <default|json>` — opencode `run`'s output format. `Json` selects
/// the raw JSON event stream the [`output`](crate::output) module parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `default` — the formatted, human-readable transcript.
    Default,
    /// `json` — the raw JSON event stream.
    Json,
}

impl OutputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Json => "json",
        }
    }
}

/// `--log-level <LEVEL>` — the log verbosity opencode writes to its log sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogLevel {
    /// `DEBUG`.
    Debug,
    /// `INFO`.
    Info,
    /// `WARN`.
    Warn,
    /// `ERROR`.
    Error,
}

impl LogLevel {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// `-m, --method <METHOD>` — the installer `opencode upgrade` should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpgradeMethod {
    /// `curl`.
    Curl,
    /// `npm`.
    Npm,
    /// `pnpm`.
    Pnpm,
    /// `bun`.
    Bun,
    /// `brew`.
    Brew,
    /// `choco`.
    Choco,
    /// `scoop`.
    Scoop,
}

impl UpgradeMethod {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Curl => "curl",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Brew => "brew",
            Self::Choco => "choco",
            Self::Scoop => "scoop",
        }
    }
}

/// `--format <json|tsv>` — the output format for `opencode db` query results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DbFormat {
    /// `json`.
    Json,
    /// `tsv`.
    Tsv,
}

impl DbFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Tsv => "tsv",
        }
    }
}

/// `--mode <all|primary|subagent>` on `opencode agent create` — the mode the
/// generated agent runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentMode {
    /// `all` — usable as both a primary agent and a subagent.
    All,
    /// `primary` — a top-level (directly invoked) agent.
    Primary,
    /// `subagent` — invoked only as a subagent.
    Subagent,
}

impl AgentMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Primary => "primary",
            Self::Subagent => "subagent",
        }
    }
}

/// `--format <table|json>` on `opencode session list` — the session-listing
/// output format. opencode defaults to `table`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionFormat {
    /// `table` — the human-readable table (opencode's default).
    Table,
    /// `json` — machine-readable JSON.
    Json,
}

impl SessionFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }
}

/// `--models` on `opencode stats` — the model-statistics display mode.
///
/// opencode's `--models` flag is value-optional: bare `--models` shows every
/// model, while `--models N` limits the table to the top `N`. This enum models
/// both shapes distinctly rather than folding them into an `Option<Option<u32>>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StatsModels {
    /// Bare `--models`: show all models.
    All,
    /// `--models N`: show only the top `N` models.
    Top(u32),
}

/// Session-continuation selection for `opencode run`, the root TUI, and
/// `opencode attach`.
///
/// opencode's `--fork` flag *requires* a session selector: `run --help`
/// documents it as "fork the session before continuing (requires --continue
/// or --session)", and yargs rejects the invocation when `--fork` is present
/// without `--continue` or `--session`. This enum makes that illegal state
/// unrepresentable — `fork` lives only on the three selector-bearing variants,
/// so `--fork` can never be emitted alone.
///
/// `--continue` and `--session <id>` together are a *valid* opencode input
/// (opencode applies the explicit id), so [`Continuation::ContinueSession`]
/// deliberately keeps that combination representable rather than collapsing
/// the two selectors into a mutually exclusive one-of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Continuation {
    /// No `--continue`/`--session`/`--fork`: start a fresh session.
    #[default]
    Fresh,
    /// `--continue [--fork]`: continue the last session.
    Continue {
        /// Emit `--fork` to fork the session before continuing.
        fork: bool,
    },
    /// `--session <id> [--fork]`: continue a specific session id.
    Session {
        /// The session id to continue.
        id: String,
        /// Emit `--fork` to fork the session before continuing.
        fork: bool,
    },
    /// `--continue --session <id> [--fork]`: both selectors set; opencode
    /// applies the explicit id (a valid input opencode accepts).
    ContinueSession {
        /// The session id to continue.
        id: String,
        /// Emit `--fork` to fork the session before continuing.
        fork: bool,
    },
}

impl Continuation {
    /// Render the continuation selector into argv: `--continue`/`--session
    /// <id>` in that order, followed by `--fork` when requested. `Fresh`
    /// renders nothing.
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Fresh => {}
            Self::Continue { fork } => {
                args.push("--continue".into());
                push_flag(args, *fork, "--fork");
            }
            Self::Session { id, fork } => {
                push_pair(args, "--session", id);
                push_flag(args, *fork, "--fork");
            }
            Self::ContinueSession { id, fork } => {
                args.push("--continue".into());
                push_pair(args, "--session", id);
                push_flag(args, *fork, "--fork");
            }
        }
    }
}
