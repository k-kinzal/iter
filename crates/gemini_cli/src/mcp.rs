//! `gemini mcp` — manage MCP servers.
//!
//! The leaf shapes are modeled from each leaf's own `gemini mcp <leaf> --help`:
//! `add` carries the full option set (`--scope` / `--transport` / `--env` /
//! `--header` / `--timeout` / `--trust` / `--description` / `--include-tools` /
//! `--exclude-tools`) plus the `[args...]` command passthrough, `remove` takes
//! `--scope`, and `enable` / `disable` take `--session`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_each, push_enum, push_flag, push_num, push_opt, push_pairs};
use crate::values::{McpScope, McpTransport};

/// `gemini mcp [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// `-d, --debug`.
    pub debug: bool,
    /// The MCP subcommand.
    pub command: McpSubcommand,
}

impl McpCommand {
    /// Wrap an MCP subcommand with default options.
    #[must_use]
    pub fn new(command: McpSubcommand) -> Self {
        Self {
            debug: false,
            command,
        }
    }
}

impl ToArgs for McpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp".into());
        push_flag(args, self.debug, "--debug");
        self.command.render(args);
    }
}

/// A `gemini mcp` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `mcp add [OPTIONS] <NAME> <COMMAND_OR_URL> [ARGS]...`.
    Add {
        /// Server name.
        name: String,
        /// Command to launch (stdio), or the server URL (sse, http).
        command_or_url: String,
        /// `-s, --scope <SCOPE>`.
        scope: Option<McpScope>,
        /// `-t, --transport <TYPE>`.
        transport: Option<McpTransport>,
        /// `-e, --env <KEY=VALUE>` (repeatable): each pair renders `key=value`.
        env: Vec<(String, String)>,
        /// `-H, --header <HEADER>` (repeatable): each pair renders `Name: value`.
        header: Vec<(String, String)>,
        /// `--timeout <MILLIS>`: connection timeout in milliseconds.
        timeout: Option<u64>,
        /// `--trust`: bypass all tool-call confirmation prompts.
        trust: bool,
        /// `--description <TEXT>`.
        description: Option<String>,
        /// `--include-tools <TOOL>` (repeatable; comma-separated list).
        include_tools: Vec<String>,
        /// `--exclude-tools <TOOL>` (repeatable; comma-separated list).
        exclude_tools: Vec<String>,
        /// Trailing `[args...]` forwarded to the stdio command.
        args: Vec<String>,
    },
    /// `mcp remove [OPTIONS] <NAME>`.
    Remove {
        /// Server name.
        name: String,
        /// `-s, --scope <SCOPE>`.
        scope: Option<McpScope>,
    },
    /// `mcp list`.
    List,
    /// `mcp enable <NAME>`.
    Enable {
        /// Server name.
        name: String,
        /// `--session`: clear session-only disable.
        session: bool,
    },
    /// `mcp disable <NAME>`.
    Disable {
        /// Server name.
        name: String,
        /// `--session`: disable for the current session only.
        session: bool,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add {
                name,
                command_or_url,
                scope,
                transport,
                env,
                header,
                timeout,
                trust,
                description,
                include_tools,
                exclude_tools,
                args: rest,
            } => {
                args.push("add".into());
                push_enum(args, "--scope", scope.map(McpScope::as_str));
                push_enum(args, "--transport", transport.map(McpTransport::as_str));
                push_pairs(args, "--env", "=", env);
                push_pairs(args, "--header", ": ", header);
                push_num(args, "--timeout", *timeout);
                push_flag(args, *trust, "--trust");
                push_opt(args, "--description", description.as_deref());
                push_each(args, "--include-tools", include_tools);
                push_each(args, "--exclude-tools", exclude_tools);
                args.push(name.into());
                args.push(command_or_url.into());
                args.extend(rest.iter().map(OsString::from));
            }
            Self::Remove { name, scope } => {
                args.push("remove".into());
                push_enum(args, "--scope", scope.map(McpScope::as_str));
                args.push(name.into());
            }
            Self::List => args.push("list".into()),
            Self::Enable { name, session } => {
                args.push("enable".into());
                args.push(name.into());
                push_flag(args, *session, "--session");
            }
            Self::Disable { name, session } => {
                args.push("disable".into());
                args.push(name.into());
                push_flag(args, *session, "--session");
            }
        }
    }
}
