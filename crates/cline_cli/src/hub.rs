//! `cline hub` — manage the local hub daemon.

use std::ffi::OsString;

use crate::args::ToArgs;

/// `cline hub <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubCommand {
    /// The hub subcommand.
    pub command: HubSubcommand,
}

impl HubCommand {
    /// Wrap a hub subcommand.
    #[must_use]
    pub fn new(command: HubSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for HubCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("hub".into());
        self.command.render(args);
    }
}

/// A `cline hub` subcommand.
///
/// The leaves take no options of their own (`-h` aside); the connection
/// options (`--host` / `--port` / `--pathname` / `--cwd`) belong to the `hub`
/// parent and are modeled as [`HubSubcommand::Raw`] passthrough when needed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HubSubcommand {
    /// `hub ensure`: start the daemon if it is not already running.
    Ensure,
    /// `hub start`: start the daemon.
    Start,
    /// `hub status`: report the daemon status.
    Status,
    /// `hub stop`: stop the daemon.
    Stop,
    /// An escape hatch for any hub invocation (including parent connection
    /// flags) not modeled above.
    Raw {
        /// Verbatim argv appended after `hub`.
        args: Vec<String>,
    },
}

impl HubSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Ensure => args.push("ensure".into()),
            Self::Start => args.push("start".into()),
            Self::Status => args.push("status".into()),
            Self::Stop => args.push("stop".into()),
            Self::Raw { args: rest } => args.extend(rest.iter().map(OsString::from)),
        }
    }
}
