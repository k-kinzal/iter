use std::ffi::OsString;

use crate::args::{push_each, push_enum, push_flag, push_opt};
use crate::values::Switch;

/// MCP configuration scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpScope {
    /// `local`.
    Local,
    /// `user`.
    User,
    /// `project`.
    Project,
}

impl McpScope {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// MCP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// `stdio`.
    Stdio,
    /// `sse`.
    Sse,
    /// `http`.
    Http,
}

impl McpTransport {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
        }
    }
}

/// `claude mcp ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mcp {
    /// `mcp add`.
    Add(McpAdd),
    /// `mcp add-from-claude-desktop`.
    AddFromClaudeDesktop(McpAddFromClaudeDesktop),
    /// `mcp add-json`.
    AddJson(McpAddJson),
    /// `mcp get`.
    Get { name: String },
    /// `mcp list`.
    List,
    /// `mcp remove`.
    Remove(McpRemove),
    /// `mcp reset-project-choices`.
    ResetProjectChoices,
    /// `mcp serve`.
    Serve(McpServe),
    /// `mcp help [command]`.
    Help(Option<McpHelpCommand>),
}

impl Mcp {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add(command) => {
                args.push("add".into());
                command.render(args);
            }
            Self::AddFromClaudeDesktop(command) => {
                args.push("add-from-claude-desktop".into());
                command.render(args);
            }
            Self::AddJson(command) => {
                args.push("add-json".into());
                command.render(args);
            }
            Self::Get { name } => {
                args.push("get".into());
                args.push(name.into());
            }
            Self::List => args.push("list".into()),
            Self::Remove(command) => {
                args.push("remove".into());
                command.render(args);
            }
            Self::ResetProjectChoices => args.push("reset-project-choices".into()),
            Self::Serve(command) => {
                args.push("serve".into());
                command.render(args);
            }
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
        }
    }
}

/// `claude mcp help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpHelpCommand {
    /// `add`.
    Add,
    /// `add-from-claude-desktop`.
    AddFromClaudeDesktop,
    /// `add-json`.
    AddJson,
    /// `get`.
    Get,
    /// `list`.
    List,
    /// `remove`.
    Remove,
    /// `reset-project-choices`.
    ResetProjectChoices,
    /// `serve`.
    Serve,
}

impl McpHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::AddFromClaudeDesktop => "add-from-claude-desktop",
            Self::AddJson => "add-json",
            Self::Get => "get",
            Self::List => "list",
            Self::Remove => "remove",
            Self::ResetProjectChoices => "reset-project-choices",
            Self::Serve => "serve",
        }
    }
}

/// `claude mcp add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAdd {
    /// Server name.
    pub name: String,
    /// Command or URL.
    pub command_or_url: String,
    /// Positional command args.
    pub args: Vec<String>,
    /// Insert `--` before `command_or_url`.
    pub separate_command: Switch,
    /// `--callback-port`.
    pub callback_port: Option<u16>,
    /// `--client-id`.
    pub client_id: Option<String>,
    /// `--client-secret`.
    ///
    /// Claude Code 2.1.178 prompts for the secret or reads
    /// `MCP_CLIENT_SECRET`; the flag itself does not take a value.
    pub client_secret: Switch,
    /// `--env`.
    pub env: Vec<String>,
    /// `--header`.
    pub headers: Vec<String>,
    /// `--scope`.
    pub scope: Option<McpScope>,
    /// `--transport`.
    pub transport: Option<McpTransport>,
}

impl McpAdd {
    /// Create an MCP add command.
    #[must_use]
    pub fn new(name: impl Into<String>, command_or_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command_or_url: command_or_url.into(),
            args: Vec::new(),
            separate_command: Switch::Off,
            callback_port: None,
            client_id: None,
            client_secret: Switch::Off,
            env: Vec::new(),
            headers: Vec::new(),
            scope: None,
            transport: None,
        }
    }

    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(
            args,
            "--callback-port",
            self.callback_port.map(|p| p.to_string()).as_deref(),
        );
        push_opt(args, "--client-id", self.client_id.as_deref());
        push_flag(args, self.client_secret, "--client-secret");
        push_each(args, "--env", &self.env);
        push_each(args, "--header", &self.headers);
        push_enum(args, "--scope", self.scope.map(McpScope::as_str));
        push_enum(
            args,
            "--transport",
            self.transport.map(McpTransport::as_str),
        );
        args.push((&self.name).into());
        if self.separate_command.is_on() || !self.args.is_empty() {
            args.push("--".into());
        }
        args.push((&self.command_or_url).into());
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `claude mcp add-from-claude-desktop`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpAddFromClaudeDesktop {
    /// `--scope`.
    pub scope: Option<McpScope>,
}

impl McpAddFromClaudeDesktop {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(McpScope::as_str));
    }
}

/// `claude mcp add-json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddJson {
    /// Server name.
    pub name: String,
    /// JSON server configuration.
    pub json: String,
    /// `--client-secret`.
    ///
    /// Claude Code 2.1.178 prompts for the secret or reads
    /// `MCP_CLIENT_SECRET`; the flag itself does not take a value.
    pub client_secret: Switch,
    /// `--scope`.
    pub scope: Option<McpScope>,
}

impl McpAddJson {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.client_secret, "--client-secret");
        push_enum(args, "--scope", self.scope.map(McpScope::as_str));
        args.push((&self.name).into());
        args.push((&self.json).into());
    }
}

/// `claude mcp remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemove {
    /// Server name.
    pub name: String,
    /// `--scope`.
    pub scope: Option<McpScope>,
}

impl McpRemove {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(McpScope::as_str));
        args.push((&self.name).into());
    }
}

/// `claude mcp serve`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServe {
    /// `--debug`.
    pub debug: Switch,
    /// `--verbose`.
    pub verbose: Switch,
}

impl McpServe {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.debug, "--debug");
        push_flag(args, self.verbose, "--verbose");
    }
}
