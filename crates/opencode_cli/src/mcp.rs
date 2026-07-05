//! `opencode mcp` — manage MCP (Model Context Protocol) servers.
//!
//! opencode 1.2.20 exposes per-leaf `--help` for the `mcp` subcommands, so the
//! leaves and the nested `mcp auth` tree are modeled by name with their typed
//! positionals. `mcp add` is the one exception: its `--help` documents no
//! positionals or flags (the CLI collects the server definition interactively),
//! so it keeps a raw-args passthrough as an escape hatch.

use std::ffi::OsString;

use crate::args::ToArgs;
use crate::options::GlobalOptions;

/// `opencode mcp <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The MCP subcommand.
    pub command: McpSubcommand,
}

impl McpCommand {
    /// Wrap an MCP subcommand with default global options.
    #[must_use]
    pub fn new(command: McpSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for McpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode mcp` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `mcp add [ARGS]...`: add an MCP server (name and transport flags).
    Add {
        /// Server identifier and any flags.
        args: Vec<String>,
    },
    /// `mcp list` (alias `ls`): list MCP servers and their status.
    List,
    /// `mcp auth <COMMAND>`: authenticate with an OAuth-enabled MCP server, or
    /// list OAuth-capable servers.
    Auth(McpAuthSubcommand),
    /// `mcp logout [name]`: remove OAuth credentials for an MCP server.
    Logout {
        /// Optional server name.
        name: Option<String>,
    },
    /// `mcp debug <name>`: debug the OAuth connection for an MCP server.
    Debug {
        /// Server name.
        name: String,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add { args: rest } => {
                args.push("add".into());
                args.extend(rest.iter().map(OsString::from));
            }
            Self::List => args.push("list".into()),
            Self::Auth(command) => {
                args.push("auth".into());
                command.render(args);
            }
            Self::Logout { name } => {
                args.push("logout".into());
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::Debug { name } => {
                args.push("debug".into());
                args.push(name.into());
            }
        }
    }
}

/// An `opencode mcp auth` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAuthSubcommand {
    /// `mcp auth [name]`: authenticate with an OAuth-enabled MCP server.
    Authenticate {
        /// Optional server name.
        name: Option<String>,
    },
    /// `mcp auth list` (alias `ls`): list OAuth-capable MCP servers and their
    /// auth status.
    List,
}

impl McpAuthSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Authenticate { name } => {
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::List => args.push("list".into()),
        }
    }
}
