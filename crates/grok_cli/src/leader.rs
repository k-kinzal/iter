//! `grok leader` — inspect and control shared leader processes.
//!
//! `grok leader <COMMAND>` groups `list`/`info`/`kill` plus the nested
//! `profile` group. The `list`/`info`/`kill` leaves and their flags are typed
//! from `grok 0.2.82`; the nested `profile` leaves (`status`/`start`/`stop`) do
//! not expose their own `--help` (asking for it prints the root help), so they
//! are modeled structurally with an `args` escape hatch.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt_num};
use crate::options::GlobalOptions;

/// A `grok leader` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderSubcommand {
    /// `list` — list running leader processes.
    List {
        /// `--json`: emit machine-readable JSON output.
        json: bool,
    },
    /// `info` — show details for a leader process.
    Info {
        /// `--pid <PID>`: leader process id (from `grok leader list`).
        pid: Option<u64>,
        /// `--json`: emit machine-readable JSON output.
        json: bool,
    },
    /// `kill` — stop all running leader processes.
    Kill,
    /// `profile <COMMAND>` — control leader profiling.
    Profile(LeaderProfileSubcommand),
}

/// A `grok leader profile` subcommand.
///
/// The `profile` leaves do not expose their own `--help` in `grok 0.2.82`
/// (asking for it prints the root help), so their flag sets are not verifiable
/// from the CLI. They are modeled structurally; append any extra flags through
/// each variant's `args`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderProfileSubcommand {
    /// `status` — show profiling status.
    Status {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `start` — start CPU profiling.
    Start {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `stop` — stop CPU profiling and write results.
    Stop {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
}

impl LeaderProfileSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("profile".into());
        let (leaf, extra) = match self {
            Self::Status { args } => ("status", args),
            Self::Start { args } => ("start", args),
            Self::Stop { args } => ("stop", args),
        };
        args.push(leaf.into());
        for arg in extra {
            args.push(arg.into());
        }
    }
}

impl LeaderSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { json } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
            }
            Self::Info { pid, json } => {
                args.push("info".into());
                push_opt_num(args, "--pid", *pid);
                push_flag(args, *json, "--json");
            }
            Self::Kill => args.push("kill".into()),
            Self::Profile(profile) => profile.render(args),
        }
    }
}

/// `grok leader [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `leader` subcommand.
    pub command: LeaderSubcommand,
}

impl LeaderCommand {
    /// Build a `leader` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: LeaderSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
        }
    }
}

impl ToArgs for LeaderCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("leader".into());
        self.global.render(args);
        self.command.render(args);
    }
}
