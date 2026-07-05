//! `grok mcp` — manage MCP server configurations.
//!
//! `grok mcp <COMMAND>` groups `list` / `add` / `remove` / `doctor`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_enum, push_flag, push_pair};
use crate::options::GlobalOptions;
use crate::values::{McpScope, McpTransport};

/// A `grok mcp` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSubcommand {
    /// `list` — list configured MCP servers.
    List {
        /// `--json`: emit machine-readable JSON output.
        json: bool,
    },
    /// `add <NAME> [COMMAND_OR_URL] [ARGS]...` — add or update an MCP server.
    Add(Box<McpAdd>),
    /// `remove <NAME>` — remove an MCP server configuration.
    Remove {
        /// Server name to remove.
        name: String,
        /// `-s, --scope <SCOPE>`: config to remove from. When omitted, all
        /// scopes are searched.
        scope: Option<McpScope>,
    },
    /// `doctor [NAME]` — diagnose MCP server configuration and connectivity.
    Doctor {
        /// Optional server name to check.
        name: Option<String>,
        /// `--json`: emit machine-readable JSON output.
        json: bool,
    },
}

/// Fields of `grok mcp add [OPTIONS] <NAME> [COMMAND_OR_URL] [ARGS]...`.
///
/// The server name is required — there is no `Default` that would yield an
/// empty name. The transport (`stdio`/`http`/`sse`) and scope (`user`/`project`)
/// are fixed-choice ([`McpTransport`] / [`McpScope`]); `env` and `header` are
/// repeatable `KEY=value` / `Name: VALUE` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAdd {
    /// `<NAME>`: server name in `config.toml`.
    pub name: String,
    /// `[COMMAND_OR_URL]`: command to launch (stdio) or URL to connect to
    /// (http/sse).
    pub command_or_url: Option<String>,
    /// `[ARGS]...`: arguments passed to the server command, rendered after `--`
    /// so server flags (e.g. `-y`) are not consumed by grok.
    pub args: Vec<String>,
    /// `-t, --transport <TRANSPORT>`: transport type (defaults to stdio).
    pub transport: Option<McpTransport>,
    /// `-s, --scope <SCOPE>`: config to write to (defaults to user).
    pub scope: Option<McpScope>,
    /// `-e, --env <KEY=value>` (repeatable): environment variables for the
    /// server process.
    pub env: Vec<(String, String)>,
    /// `-H, --header <NAME: VALUE>` (repeatable): HTTP headers for remote
    /// servers.
    pub header: Vec<(String, String)>,
}

impl McpAdd {
    /// Build an `mcp add` for a server `name`, with all other fields defaulted.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command_or_url: None,
            args: Vec::new(),
            transport: None,
            scope: None,
            env: Vec::new(),
            header: Vec::new(),
        }
    }

    fn render(&self, args: &mut Vec<OsString>) {
        push_enum(
            args,
            "--transport",
            self.transport.map(McpTransport::as_str),
        );
        push_enum(args, "--scope", self.scope.map(McpScope::as_str));
        for (key, value) in &self.env {
            push_pair(args, "--env", format!("{key}={value}"));
        }
        for (name, value) in &self.header {
            push_pair(args, "--header", format!("{name}: {value}"));
        }
        args.push((&self.name).into());
        // `[ARGS]...` must sit after `--` so server flags (e.g. `-y`) are not
        // parsed by grok; the command/url positional rides in front of them.
        if self.args.is_empty() {
            if let Some(command_or_url) = &self.command_or_url {
                args.push(command_or_url.into());
            }
        } else {
            args.push("--".into());
            if let Some(command_or_url) = &self.command_or_url {
                args.push(command_or_url.into());
            }
            for arg in &self.args {
                args.push(arg.into());
            }
        }
    }
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { json } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
            }
            Self::Add(add) => {
                args.push("add".into());
                add.render(args);
            }
            Self::Remove { name, scope } => {
                args.push("remove".into());
                push_enum(args, "--scope", scope.map(McpScope::as_str));
                args.push(name.into());
            }
            Self::Doctor { name, json } => {
                args.push("doctor".into());
                push_flag(args, *json, "--json");
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
        }
    }
}

/// `grok mcp [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `mcp` subcommand.
    pub command: McpSubcommand,
}

impl McpCommand {
    /// Build an `mcp` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: McpSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
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
