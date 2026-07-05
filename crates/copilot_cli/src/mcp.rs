//! `copilot mcp` — manage MCP server configuration.
//!
//! The `mcp` root takes no options of its own; each leaf (`add`/`get`/`list`/
//! `remove`) is modeled with its verified flag set. `add` carries the richest
//! surface: a [`McpTransport`] one-of that pairs the transport with its
//! *required* target (a URL for the remote `http`/`sse` transports, or — for
//! the default `stdio` transport — a local command after `--` plus its `--env`
//! subprocess variables), plus the shared [`McpAddOptions`] (`--header`,
//! `--tools`, `--timeout`, `--json`, `--show-secrets`).

use std::ffi::OsString;

use crate::args::{ToArgs, push_each, push_enum, push_flag, push_opt, push_pair};

/// The transport of a `copilot mcp add` and its *required* target.
///
/// Copilot's `mcp add` positional is either a remote URL or, after `--`, a
/// local command and its arguments — and which one is required is decided by
/// the transport. Modeling the transport and its target as a single one-of
/// makes the missing-target state (a remote transport with no URL, or `stdio`
/// with no command) unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// `stdio` (Copilot's default): a local subprocess speaking over stdio,
    /// launched as `mcp add <name> -- <command> [args...]`.
    Stdio {
        /// The local command to launch (the first token after `--`).
        command: String,
        /// Arguments passed to the command (the remaining tokens after `--`).
        args: Vec<String>,
        /// `--env <KEY=VALUE>` (repeatable): subprocess environment variables,
        /// each rendered `key=value`.
        env: Vec<(String, String)>,
    },
    /// `http`: a remote streamable-HTTP endpoint at the given URL.
    Http {
        /// The remote endpoint URL positional.
        url: String,
    },
    /// `sse`: a remote server-sent-events endpoint at the given URL.
    Sse {
        /// The remote endpoint URL positional.
        url: String,
    },
}

impl McpTransport {
    /// The `--transport` value to emit, or `None` for the default `stdio`
    /// transport (which needs no explicit flag; its `--` command disambiguates
    /// it from the remote-URL form).
    #[must_use]
    pub(crate) const fn transport_flag(&self) -> Option<&'static str> {
        match self {
            Self::Stdio { .. } => None,
            Self::Http { .. } => Some("http"),
            Self::Sse { .. } => Some("sse"),
        }
    }

    /// Render the transport-specific flags that precede `<name>` (currently
    /// only the `stdio` transport's `--env` pairs).
    fn render_flags(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--transport", self.transport_flag());
        if let Self::Stdio { env, .. } = self {
            for (key, value) in env {
                push_pair(args, "--env", format!("{key}={value}"));
            }
        }
    }

    /// Render the positional target that follows `<name>`: the remote URL, or
    /// `-- <command> [args...]` for the `stdio` transport.
    fn render_target(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Http { url } | Self::Sse { url } => args.push(url.into()),
            Self::Stdio {
                command,
                args: rest,
                ..
            } => {
                args.push("--".into());
                args.push(command.into());
                args.extend(rest.iter().map(OsString::from));
            }
        }
    }
}

/// Options shared across every `copilot mcp add` transport.
///
/// The transport and its required target live in [`McpTransport`]; this struct
/// carries only the flags that apply regardless of transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpAddOptions {
    /// `--header <header>` (repeatable, remote servers).
    pub headers: Vec<String>,
    /// `--tools <tools>`: `"*"`, a comma-separated list, or `""` for none.
    pub tools: Option<String>,
    /// `--timeout <ms>`.
    pub timeout_ms: Option<u64>,
    /// `--show-secrets`: reveal env-var and header values in output.
    pub show_secrets: bool,
    /// `--json`: output the added config as JSON.
    pub json: bool,
}

impl McpAddOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_each(args, "--header", &self.headers);
        push_opt(args, "--tools", self.tools.as_deref());
        if let Some(timeout) = self.timeout_ms {
            args.push("--timeout".into());
            args.push(timeout.to_string().into());
        }
        push_flag(args, self.show_secrets, "--show-secrets");
        push_flag(args, self.json, "--json");
    }
}

/// `copilot mcp <COMMAND>`.
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

/// A `copilot mcp` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `mcp add [OPTIONS] <NAME> <URL-OR-COMMAND-AND-ARGS>...`.
    Add {
        /// Server name.
        name: String,
        /// The transport and its required target (remote URL, or the local
        /// command after `--`).
        transport: McpTransport,
        /// `add`-specific options shared across transports.
        options: McpAddOptions,
    },
    /// `mcp get [OPTIONS] <NAME>`.
    Get {
        /// Server name.
        name: String,
        /// `--json`: output as JSON.
        json: bool,
        /// `--show-secrets`.
        show_secrets: bool,
    },
    /// `mcp list [--json]`.
    List {
        /// `--json`: machine-readable output.
        json: bool,
    },
    /// `mcp remove <NAME>`.
    Remove {
        /// Server name.
        name: String,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add {
                name,
                transport,
                options,
            } => {
                args.push("add".into());
                transport.render_flags(args);
                options.render(args);
                args.push(name.into());
                transport.render_target(args);
            }
            Self::Get {
                name,
                json,
                show_secrets,
            } => {
                args.push("get".into());
                push_flag(args, *json, "--json");
                push_flag(args, *show_secrets, "--show-secrets");
                args.push(name.into());
            }
            Self::List { json } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
            }
            Self::Remove { name } => {
                args.push("remove".into());
                args.push(name.into());
            }
        }
    }
}
