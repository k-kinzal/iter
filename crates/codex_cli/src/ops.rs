//! Operational and experimental subcommands.
//!
//! `sandbox`, `apply`, `completion`, `update`, `doctor`, `debug`, and
//! `mcp-server` cover local tooling; `exec-server`, `remote-control`, and `app`
//! are marked `[experimental]`/`[EXPERIMENTAL]` by Codex but still have typed
//! flags. Only the `app-server` and `cloud` commands retain a raw-args
//! passthrough for their deep nested trees (the `debug app-server` tree
//! likewise stays a passthrough).

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_each, push_flag, push_opt, push_opt_path};
use crate::options::GlobalConfig;
use crate::values::CompletionShell;

/// `codex sandbox [OPTIONS] [COMMAND]...` — run a command under Codex's
/// platform sandbox (seatbelt on macOS, landlock on Linux).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `-P, --permissions-profile <NAME>`.
    pub permissions_profile: Option<String>,
    /// `-p, --profile <CONFIG_PROFILE>`.
    pub profile: Option<String>,
    /// `-C, --cd <DIR>`.
    pub cd: Option<PathBuf>,
    /// `--include-managed-config`: include managed requirements while resolving
    /// an explicit permissions profile.
    pub include_managed_config: bool,
    /// `--allow-unix-socket <ALLOW_UNIX_SOCKETS>` (repeatable): allow the
    /// sandboxed command to bind/connect `AF_UNIX` sockets rooted at this path.
    pub allow_unix_sockets: Vec<String>,
    /// `--log-denials`: capture macOS sandbox denials via `log stream`.
    pub log_denials: bool,
    /// The sandboxed command and its arguments.
    pub command: Vec<String>,
}

impl ToArgs for SandboxCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("sandbox".into());
        self.global.render(args);
        push_opt(
            args,
            "--permissions-profile",
            self.permissions_profile.as_deref(),
        );
        push_opt(args, "--profile", self.profile.as_deref());
        push_opt_path(args, "--cd", self.cd.as_deref());
        push_flag(
            args,
            self.include_managed_config,
            "--include-managed-config",
        );
        push_each(args, "--allow-unix-socket", &self.allow_unix_sockets);
        push_flag(args, self.log_denials, "--log-denials");
        if !self.command.is_empty() {
            args.push("--".into());
            args.extend(self.command.iter().map(OsString::from));
        }
    }
}

/// `codex apply [OPTIONS] <TASK_ID>` — apply the latest agent diff via
/// `git apply`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// The task id whose diff to apply.
    pub task_id: String,
}

impl ToArgs for ApplyCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("apply".into());
        self.global.render(args);
        args.push((&self.task_id).into());
    }
}

/// `codex completion [OPTIONS] [SHELL]` — generate a shell completion script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// Target shell (Codex defaults to `bash` when omitted).
    pub shell: Option<CompletionShell>,
}

impl ToArgs for CompletionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("completion".into());
        self.global.render(args);
        // `completion` takes the shell as a bare positional, not a flag.
        if let Some(shell) = self.shell {
            args.push(shell.as_str().into());
        }
    }
}

/// `codex update [OPTIONS]` — self-update Codex.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
}

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
        self.global.render(args);
    }
}

/// `codex doctor [OPTIONS]` — diagnose the local installation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--json`: emit a redacted machine-readable report.
    pub json: bool,
    /// `--summary`: only show grouped rows and the final count.
    pub summary: bool,
    /// `--all`: expand long lists in detailed output.
    pub all: bool,
    /// `--no-color`: disable ANSI color.
    pub no_color: bool,
    /// `--ascii`: use ASCII status labels and separators in human output.
    pub ascii: bool,
}

impl ToArgs for DoctorCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("doctor".into());
        self.global.render(args);
        push_flag(args, self.json, "--json");
        push_flag(args, self.summary, "--summary");
        push_flag(args, self.all, "--all");
        push_flag(args, self.no_color, "--no-color");
        push_flag(args, self.ascii, "--ascii");
    }
}

/// `codex mcp-server [OPTIONS]` — run Codex as a stdio MCP server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--strict-config`.
    pub strict_config: bool,
}

impl ToArgs for McpServerCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("mcp-server".into());
        self.global.render(args);
        push_flag(args, self.strict_config, "--strict-config");
    }
}

/// `codex debug <COMMAND>` — developer debugging tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// The debug subcommand.
    pub command: DebugSubcommand,
}

impl ToArgs for DebugCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("debug".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// A `codex debug` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugSubcommand {
    /// `debug models`: render the raw model catalog as JSON.
    Models,
    /// `debug prompt-input`: render the model-visible prompt input as JSON.
    PromptInput,
    /// `debug app-server [ARGS]...`.
    AppServer {
        /// Passthrough args.
        args: Vec<String>,
    },
}

impl DebugSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Models => args.push("models".into()),
            Self::PromptInput => args.push("prompt-input".into()),
            Self::AppServer { args: rest } => {
                args.push("app-server".into());
                args.extend(rest.iter().map(OsString::from));
            }
        }
    }
}

/// `codex app-server [OPTIONS] [COMMAND]...` — **experimental**.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppServerCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--strict-config`.
    pub strict_config: bool,
    /// Nested subcommand (`daemon`/`proxy`/`generate-ts`/…) and its args.
    pub args: Vec<String>,
}

impl ToArgs for AppServerCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("app-server".into());
        self.global.render(args);
        push_flag(args, self.strict_config, "--strict-config");
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `codex cloud [OPTIONS] [COMMAND]...` — **experimental**.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// Nested subcommand (`exec`/`status`/`list`/`apply`/`diff`) and its args.
    pub args: Vec<String>,
}

impl ToArgs for CloudCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("cloud".into());
        self.global.render(args);
        args.extend(self.args.iter().map(OsString::from));
    }
}

/// `codex exec-server [OPTIONS]` — **experimental**.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecServerCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--strict-config`.
    pub strict_config: bool,
    /// `--listen <URL>`.
    pub listen: Option<String>,
    /// `--remote <URL>`.
    pub remote: Option<String>,
    /// `--environment-id <ID>`.
    pub environment_id: Option<String>,
    /// `--name <NAME>`: human-readable environment name.
    pub name: Option<String>,
    /// `--use-agent-identity-auth`: use Agent Identity auth from
    /// `CODEX_ACCESS_TOKEN` for remote registration.
    pub use_agent_identity_auth: bool,
}

impl ToArgs for ExecServerCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("exec-server".into());
        self.global.render(args);
        push_flag(args, self.strict_config, "--strict-config");
        push_opt(args, "--listen", self.listen.as_deref());
        push_opt(args, "--remote", self.remote.as_deref());
        push_opt(args, "--environment-id", self.environment_id.as_deref());
        push_opt(args, "--name", self.name.as_deref());
        push_flag(
            args,
            self.use_agent_identity_auth,
            "--use-agent-identity-auth",
        );
    }
}

/// `codex remote-control [OPTIONS] [COMMAND]` — **experimental**.
///
/// Both leaves (`start`, `stop`) accept only the parent's `--json` and config
/// family, so they are modeled as a bare [`RemoteControlSubcommand`] enum.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteControlCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--json`: emit machine-readable JSON.
    pub json: bool,
    /// Optional `start`/`stop` subcommand.
    pub command: Option<RemoteControlSubcommand>,
}

impl ToArgs for RemoteControlCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("remote-control".into());
        self.global.render(args);
        push_flag(args, self.json, "--json");
        if let Some(command) = &self.command {
            command.render(args);
        }
    }
}

/// A `codex remote-control` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteControlSubcommand {
    /// `remote-control start`: start the app-server daemon with remote control.
    Start,
    /// `remote-control stop`: stop the app-server daemon.
    Stop,
}

impl RemoteControlSubcommand {
    fn render(self, args: &mut Vec<OsString>) {
        match self {
            Self::Start => args.push("start".into()),
            Self::Stop => args.push("stop".into()),
        }
    }
}

/// `codex app [OPTIONS] [PATH]` — launch the Codex desktop app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--download-url <URL>`: override the installer download URL.
    pub download_url: Option<String>,
    /// Optional workspace `[PATH]` positional (Codex defaults to `.`).
    pub path: Option<PathBuf>,
}

impl ToArgs for AppCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("app".into());
        self.global.render(args);
        push_opt(args, "--download-url", self.download_url.as_deref());
        if let Some(path) = &self.path {
            args.push(path.into());
        }
    }
}
