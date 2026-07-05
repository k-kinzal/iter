//! `hermes mcp <command>` — MCP server management, and running Hermes as an
//! MCP server.
//!
//! The `rm` / `ls` / `config` aliases collapse onto the canonical
//! `remove` / `list` / `configure` forms modeled here.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_pair, push_positional};
use crate::values::McpAuth;

/// Which transport an MCP server uses.
///
/// A server is exactly one of the CLI's three mutually exclusive kinds — an
/// HTTP/SSE URL, a stdio command, or a known preset. Supplying none is
/// incomplete and supplying two is contradictory, so the choice is a single
/// enum rather than several independent options. The stdio-only `--env` and
/// `--args` therefore live inside [`Stdio`](Self::Stdio) and cannot attach to
/// the other transports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// `--url <URL>`: an HTTP/SSE endpoint.
    Http {
        /// The endpoint URL.
        url: String,
    },
    /// `--command <MCP_COMMAND>`: a stdio server (e.g. `npx`), with its
    /// environment and argument list.
    Stdio {
        /// The stdio command (e.g. `npx`).
        command: String,
        /// `--env <KEY=VALUE>` (repeatable): environment variables for the
        /// stdio server, rendered as `KEY=VALUE`.
        env: Vec<(String, String)>,
        /// `--args ...`: verbatim arguments for the stdio command. Rendered
        /// last, as the CLI requires (`--args` consumes the remainder of argv).
        args: Vec<String>,
    },
    /// `--preset <PRESET>`: a known MCP preset name.
    Preset(String),
}

/// Options for `hermes mcp add <NAME>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddOptions {
    /// The server name (used as the config key).
    pub name: String,
    /// The server transport (exactly one of URL / stdio command / preset).
    pub transport: McpTransport,
    /// `--auth <METHOD>`: auth method (for remote HTTP/SSE servers).
    pub auth: Option<McpAuth>,
}

impl McpAddOptions {
    /// Options for `hermes mcp add <NAME>` with the given transport.
    #[must_use]
    pub fn new(name: impl Into<String>, transport: McpTransport) -> Self {
        Self {
            name: name.into(),
            transport,
            auth: None,
        }
    }
}

/// A `hermes mcp` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpSubcommand {
    /// `serve`: run Hermes as an MCP server.
    Serve {
        /// `-v` / `--verbose`: verbose logging on stderr.
        verbose: bool,
        /// `--accept-hooks`: auto-approve unseen shell hooks.
        accept_hooks: bool,
    },
    /// `add <NAME>`: add an MCP server.
    Add(McpAddOptions),
    /// `remove <NAME>` (alias `rm`): remove an MCP server.
    Remove {
        /// The server to remove.
        name: String,
    },
    /// `list` (alias `ls`): list configured MCP servers.
    List,
    /// `test <NAME>`: test an MCP server connection.
    Test {
        /// The server to test.
        name: String,
    },
    /// `configure <NAME>` (alias `config`): toggle tool selection.
    Configure {
        /// The server to configure.
        name: String,
    },
    /// `login <NAME>`: force re-authentication for an OAuth-based MCP server.
    Login {
        /// The server to re-authenticate.
        name: String,
    },
    /// `picker`: the interactive catalog picker.
    Picker,
    /// `catalog`: list Nous-approved MCPs available for one-click install.
    Catalog,
    /// `install <IDENTIFIER>`: install a catalog MCP by name.
    Install {
        /// The catalog entry name (or `official/<name>`).
        identifier: String,
    },
}

impl McpSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Serve {
                verbose,
                accept_hooks,
            } => {
                args.push("serve".into());
                push_flag(args, *verbose, "--verbose");
                push_flag(args, *accept_hooks, "--accept-hooks");
            }
            Self::Add(options) => {
                args.push("add".into());
                push_positional(args, &options.name);
                if let Some(auth) = options.auth {
                    push_pair(args, "--auth", auth.as_str());
                }
                match &options.transport {
                    McpTransport::Http { url } => push_pair(args, "--url", url),
                    McpTransport::Preset(preset) => push_pair(args, "--preset", preset),
                    McpTransport::Stdio {
                        command,
                        env,
                        args: cmd_args,
                    } => {
                        push_pair(args, "--command", command);
                        for (key, value) in env {
                            args.push("--env".into());
                            args.push(format!("{key}={value}").into());
                        }
                        // `--args` consumes the remainder, so it must render last.
                        if !cmd_args.is_empty() {
                            args.push("--args".into());
                            for arg in cmd_args {
                                args.push(arg.into());
                            }
                        }
                    }
                }
            }
            Self::Remove { name } => {
                args.push("remove".into());
                push_positional(args, name);
            }
            Self::List => args.push("list".into()),
            Self::Test { name } => {
                args.push("test".into());
                push_positional(args, name);
            }
            Self::Configure { name } => {
                args.push("configure".into());
                push_positional(args, name);
            }
            Self::Login { name } => {
                args.push("login".into());
                push_positional(args, name);
            }
            Self::Picker => args.push("picker".into()),
            Self::Catalog => args.push("catalog".into()),
            Self::Install { identifier } => {
                args.push("install".into());
                push_positional(args, identifier);
            }
        }
    }
}

/// `hermes mcp <command>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCommand {
    /// The leaf subcommand.
    pub subcommand: McpSubcommand,
}

impl McpCommand {
    /// Wrap an [`McpSubcommand`].
    #[must_use]
    pub fn new(subcommand: McpSubcommand) -> Self {
        Self { subcommand }
    }

    /// `hermes mcp list`.
    #[must_use]
    pub fn list() -> Self {
        Self::new(McpSubcommand::List)
    }

    /// `hermes mcp add <NAME>` with the given transport.
    #[must_use]
    pub fn add(name: impl Into<String>, transport: McpTransport) -> Self {
        Self::new(McpSubcommand::Add(McpAddOptions::new(name, transport)))
    }

    /// `hermes mcp remove <NAME>`.
    #[must_use]
    pub fn remove(name: impl Into<String>) -> Self {
        Self::new(McpSubcommand::Remove { name: name.into() })
    }
}

impl ToArgs for McpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp".into());
        self.subcommand.render(args);
    }
}
