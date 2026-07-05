//! Small management commands: `config`, `dashboard`, `doctor`, `hook`,
//! `kanban`, `mcp`, `update`, and `version`.
//!
//! These share no family structure, so they live together here rather than in
//! one module each — the same grouping `codex_cli` uses for its odds and ends.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_num, push_opt_path};

/// `cline config`: print the resolved configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigCommand {
    /// `--json`: output as JSON.
    pub json: bool,
    /// `--config <dir>`: configuration directory.
    pub config: Option<PathBuf>,
}

impl ToArgs for ConfigCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("config".into());
        push_flag(args, self.json, "--json");
        push_opt_path(args, "--config", self.config.as_deref());
    }
}

/// `cline doctor [COMMAND]`: diagnose the installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCommand {
    /// The doctor subcommand (`Report` is the bare `cline doctor`).
    pub command: DoctorSubcommand,
}

impl DoctorCommand {
    /// Wrap a doctor subcommand.
    #[must_use]
    pub fn new(command: DoctorSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for DoctorCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("doctor".into());
        self.command.render(args);
    }
}

/// A `cline doctor` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DoctorSubcommand {
    /// The bare `cline doctor`: run diagnostics.
    Report {
        /// `--cwd <path>`: working directory to diagnose.
        cwd: Option<PathBuf>,
        /// `--json`: output as JSON.
        json: bool,
        /// `--verbose`: show verbose output.
        verbose: bool,
    },
    /// `doctor fix`: apply automatic fixes.
    Fix {
        /// `--cwd <path>`: working directory to repair.
        cwd: Option<PathBuf>,
        /// `--json`: output as JSON.
        json: bool,
        /// `--verbose`: show verbose output.
        verbose: bool,
    },
    /// `doctor log`: show the diagnostics log.
    Log,
}

impl DoctorSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Report { cwd, json, verbose } => {
                push_opt_path(args, "--cwd", cwd.as_deref());
                push_flag(args, *json, "--json");
                push_flag(args, *verbose, "--verbose");
            }
            Self::Fix { cwd, json, verbose } => {
                args.push("fix".into());
                push_opt_path(args, "--cwd", cwd.as_deref());
                push_flag(args, *json, "--json");
                push_flag(args, *verbose, "--verbose");
            }
            Self::Log => args.push("log".into()),
        }
    }
}

/// `cline dashboard`: launch the web dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardCommand {
    /// `--cwd <path>`: working directory.
    pub cwd: Option<PathBuf>,
    /// `--host <host>`: bind host.
    pub host: Option<String>,
    /// `--port <port>`: bind port.
    pub port: Option<u16>,
    /// `--public-url <url>`: externally reachable URL.
    pub public_url: Option<String>,
    /// `--room-secret <secret>`: shared room secret.
    pub room_secret: Option<String>,
    /// `--no-open`: do not open a browser window.
    pub no_open: bool,
}

impl ToArgs for DashboardCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("dashboard".into());
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_opt(args, "--host", self.host.as_deref());
        push_opt_num(args, "--port", self.port);
        push_opt(args, "--public-url", self.public_url.as_deref());
        push_opt(args, "--room-secret", self.room_secret.as_deref());
        push_flag(args, self.no_open, "--no-open");
    }
}

/// `cline update`: update the CLI to the latest version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand {
    /// `--verbose`: show verbose output.
    pub verbose: bool,
    /// `--config <dir>`: configuration directory.
    pub config: Option<PathBuf>,
}

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
        push_flag(args, self.verbose, "--verbose");
        push_opt_path(args, "--config", self.config.as_deref());
    }
}

/// `cline mcp [ARGS]...`: manage MCP servers.
///
/// The `mcp` command dispatches an interactive wizard and a family of
/// server-management verbs whose surface Cline only prints under
/// `mcp --help`. This builder passes its argv through verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpCommand {
    /// Verbatim argv appended after `mcp`.
    pub args: Vec<String>,
}

impl ToArgs for McpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp".into());
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `cline hook`: internal hook entry point (invoked by Cline itself).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookCommand {
    /// Verbatim argv appended after `hook`.
    pub args: Vec<String>,
}

impl ToArgs for HookCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("hook".into());
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `cline kanban`: open the kanban board.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KanbanCommand {
    /// Verbatim argv appended after `kanban`.
    pub args: Vec<String>,
}

impl ToArgs for KanbanCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("kanban".into());
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `cline version`: print the CLI version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionCommand;

impl ToArgs for VersionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("version".into());
    }
}
