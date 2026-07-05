//! `cursor-agent mcp` — manage MCP servers.

use std::ffi::OsString;

use crate::args::ToArgs;

/// `cursor-agent mcp <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// The MCP subcommand.
    pub command: McpSubcommand,
}

impl McpCommand {
    /// Wrap an MCP subcommand.
    #[must_use]
    pub fn new(command: McpSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for McpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp".into());
        self.command.render(args);
    }
}

/// A `cursor-agent mcp` subcommand.
///
/// The identifier refers to a server configured in `.cursor/mcp.json` or
/// `~/.cursor/mcp.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `mcp login <identifier>` — authenticate with a configured MCP server.
    Login {
        /// Server identifier.
        identifier: String,
    },
    /// `mcp list` — list configured MCP servers and their status.
    List,
    /// `mcp list-tools <identifier>` — list a server's tools and argument
    /// names.
    ListTools {
        /// Server identifier.
        identifier: String,
    },
    /// `mcp enable <identifier>` — add a server to the local approved list.
    Enable {
        /// Server identifier.
        identifier: String,
    },
    /// `mcp disable <identifier>` — disable a server.
    Disable {
        /// Server identifier.
        identifier: String,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List => args.push("list".into()),
            Self::Login { identifier } => {
                args.push("login".into());
                args.push(identifier.into());
            }
            Self::ListTools { identifier } => {
                args.push("list-tools".into());
                args.push(identifier.into());
            }
            Self::Enable { identifier } => {
                args.push("enable".into());
                args.push(identifier.into());
            }
            Self::Disable { identifier } => {
                args.push("disable".into());
                args.push(identifier.into());
            }
        }
    }
}
