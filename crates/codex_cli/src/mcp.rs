//! `codex mcp` — manage external MCP servers.
//!
//! Each leaf (`list`, `get`, `add`, `remove`, `login`, `logout`) has its own
//! `--help`, so the leaf shapes are modeled with typed positionals and flags
//! rather than a raw-args passthrough. The config family (`-c/--config`,
//! `--enable`, `--disable`) is accepted at the `mcp` parent level and rendered
//! via [`GlobalConfig`].

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt, push_pair};
use crate::options::GlobalConfig;

/// `codex mcp [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// The MCP subcommand.
    pub command: McpSubcommand,
}

impl McpCommand {
    /// Wrap an MCP subcommand with default global options.
    #[must_use]
    pub fn new(command: McpSubcommand) -> Self {
        Self {
            global: GlobalConfig::default(),
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

/// A `codex mcp` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `mcp list [--json]`.
    List {
        /// `--json`: output the configured servers as JSON.
        json: bool,
    },
    /// `mcp get [--json] <NAME>`.
    Get {
        /// Server name.
        name: String,
        /// `--json`: output the server configuration as JSON.
        json: bool,
    },
    /// `mcp add [OPTIONS] <NAME> (--url <URL> | -- <COMMAND>...)`.
    ///
    /// The transport is a one-of: Codex's usage `<NAME> (--url <URL> | --
    /// <COMMAND>...)` is a clap exactly-one group, so emitting neither or both
    /// is a parse error. Modeling the transport as [`McpTransport`] makes both
    /// illegal states unrepresentable and ties the conditional flags to their
    /// documented transport (`--env` to stdio, bearer/OAuth to HTTP).
    Add {
        /// Name for the MCP server configuration.
        name: String,
        /// Transport-specific arguments (stdio command / HTTP url + auth).
        transport: McpTransport,
    },
    /// `mcp remove <NAME>`.
    Remove {
        /// Server name.
        name: String,
    },
    /// `mcp login [--scopes <SCOPE,SCOPE>] <NAME>`.
    Login {
        /// Server name.
        name: String,
        /// `--scopes <SCOPE,SCOPE>`: comma-separated OAuth scopes to request.
        scopes: Option<String>,
    },
    /// `mcp logout <NAME>`.
    Logout {
        /// Server name.
        name: String,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { json } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
            }
            Self::Get { name, json } => {
                args.push("get".into());
                push_flag(args, *json, "--json");
                args.push(name.into());
            }
            Self::Add { name, transport } => {
                args.push("add".into());
                args.push(name.into());
                transport.render(args);
            }
            Self::Remove { name } => {
                args.push("remove".into());
                args.push(name.into());
            }
            Self::Login { name, scopes } => {
                args.push("login".into());
                push_opt(args, "--scopes", scopes.as_deref());
                args.push(name.into());
            }
            Self::Logout { name } => {
                args.push("logout".into());
                args.push(name.into());
            }
        }
    }
}

/// Transport for `mcp add`: exactly one of a stdio server (`-- <command>...`)
/// or a streamable HTTP server (`--url <URL>`).
///
/// Codex's `mcp add` usage `<NAME> (--url <URL> | -- <COMMAND>...)` is a clap
/// exactly-one group — neither-set and both-set are both parse errors. Encoding
/// the transport as a one-of makes those illegal states unrepresentable, and
/// nesting each transport's conditional flags under the matching variant honors
/// the documented "Only valid with stdio servers" / "Only valid with streamable
/// HTTP servers" restrictions.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// A stdio MCP server launched via the trailing `-- <command> [args...]`
    /// group. `command` is a required non-empty first token, so the `--
    /// COMMAND` group can never be empty. `--env <KEY=VALUE>` is only valid
    /// here ("Only valid with stdio servers").
    Stdio {
        /// The executable to launch (the first `-- COMMAND` token).
        command: String,
        /// Additional arguments passed after `command`.
        args: Vec<String>,
        /// `--env <KEY=VALUE>` pairs (repeatable), rendered as `KEY=VALUE`.
        env: Vec<(String, String)>,
    },
    /// A streamable HTTP MCP server addressed by `--url <URL>`, with the
    /// HTTP-only auth flags ("Only valid with streamable HTTP servers").
    Http {
        /// `--url <URL>`: URL for the streamable HTTP MCP server.
        url: String,
        /// `--bearer-token-env-var <ENV_VAR>`: env var to read for a bearer
        /// token.
        bearer_token_env_var: Option<String>,
        /// `--oauth-client-id <CLIENT_ID>`.
        oauth_client_id: Option<String>,
        /// `--oauth-resource <RESOURCE>`.
        oauth_resource: Option<String>,
    },
}

impl McpTransport {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Stdio {
                command,
                args: command_args,
                env,
            } => {
                for (key, value) in env {
                    push_pair(args, "--env", format!("{key}={value}"));
                }
                args.push("--".into());
                args.push(command.into());
                args.extend(command_args.iter().map(OsString::from));
            }
            Self::Http {
                url,
                bearer_token_env_var,
                oauth_client_id,
                oauth_resource,
            } => {
                push_pair(args, "--url", url);
                push_opt(
                    args,
                    "--bearer-token-env-var",
                    bearer_token_env_var.as_deref(),
                );
                push_opt(args, "--oauth-client-id", oauth_client_id.as_deref());
                push_opt(args, "--oauth-resource", oauth_resource.as_deref());
            }
        }
    }
}
