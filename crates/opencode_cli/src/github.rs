//! `opencode github` — manage the GitHub agent.

use std::ffi::OsString;

use crate::args::{ToArgs, push_opt};
use crate::options::GlobalOptions;

/// `opencode github <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The github subcommand.
    pub command: GithubSubcommand,
}

impl GithubCommand {
    /// Wrap a github subcommand with default global options.
    #[must_use]
    pub fn new(command: GithubSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for GithubCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("github".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode github` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GithubSubcommand {
    /// `github install`: install the GitHub agent.
    Install,
    /// `github run`: run the GitHub agent.
    Run {
        /// `--event <EVENT>`: the GitHub mock event to run the agent for.
        event: Option<String>,
        /// `--token <TOKEN>`: GitHub personal access token (`github_pat_*`).
        token: Option<String>,
    },
}

impl GithubSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Install => args.push("install".into()),
            Self::Run { event, token } => {
                args.push("run".into());
                push_opt(args, "--event", event.as_deref());
                push_opt(args, "--token", token.as_deref());
            }
        }
    }
}
