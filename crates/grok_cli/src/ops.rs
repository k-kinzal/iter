//! Operational leaves: `completions`, `dashboard`, `inspect`, `models`,
//! `setup`, `update`, `version`, `wrap`.
//!
//! These are flat top-level commands with no further nesting, gathered here
//! rather than each in its own module.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt};
use crate::options::GlobalOptions;
use crate::values::CompletionShell;

/// `grok completions <SHELL>` — generate a shell completion script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionsCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// Shell to generate completions for.
    pub shell: CompletionShell,
}

impl CompletionsCommand {
    /// Generate completions for `shell`.
    #[must_use]
    pub fn new(shell: CompletionShell) -> Self {
        Self {
            global: GlobalOptions::default(),
            shell,
        }
    }
}

impl ToArgs for CompletionsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("completions".into());
        self.global.render(args);
        args.push(self.shell.as_str().into());
    }
}

/// `grok dashboard` — open the usage dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
}

impl ToArgs for DashboardCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("dashboard".into());
        self.global.render(args);
    }
}

/// `grok inspect` — inspect the current environment and configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// `--json`: emit machine-readable JSON output.
    pub json: bool,
}

impl ToArgs for InspectCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("inspect".into());
        self.global.render(args);
        push_flag(args, self.json, "--json");
    }
}

/// `grok models` — list available models.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
}

impl ToArgs for ModelsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("models".into());
        self.global.render(args);
    }
}

/// `grok setup` — run the interactive setup flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
}

impl ToArgs for SetupCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("setup".into());
        self.global.render(args);
    }
}

/// `grok update` — update the Grok CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// `--check`: check for an update without installing it.
    pub check: bool,
    /// `--json`: emit machine-readable JSON output.
    pub json: bool,
    /// `--force-reinstall`: reinstall even if already up to date.
    pub force_reinstall: bool,
    /// `--version <VERSION>`: install a specific version.
    pub version: Option<String>,
    /// `--alpha`: use the alpha release channel.
    pub alpha: bool,
    /// `--stable`: use the stable release channel.
    pub stable: bool,
}

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
        self.global.render(args);
        push_flag(args, self.check, "--check");
        push_flag(args, self.json, "--json");
        push_flag(args, self.force_reinstall, "--force-reinstall");
        push_opt(args, "--version", self.version.as_deref());
        push_flag(args, self.alpha, "--alpha");
        push_flag(args, self.stable, "--stable");
    }
}

/// `grok version` — print version information (alias `v`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// `--json`: emit machine-readable JSON output.
    pub json: bool,
}

impl ToArgs for VersionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("version".into());
        self.global.render(args);
        push_flag(args, self.json, "--json");
    }
}

/// `grok wrap <CMD>...` — run a command in a local PTY that forwards its
/// clipboard (OSC 52) to the system clipboard.
///
/// The `<CMD>...` positional requires at least one value, so the command is
/// modeled as a required head plus any further `args`; an empty `wrap` (which
/// grok rejects as a missing-argument usage error) is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The command to run (the required head of `<CMD>...`).
    pub command: String,
    /// Arguments passed to the wrapped command.
    pub args: Vec<String>,
}

impl WrapCommand {
    /// Wrap `command` with no extra arguments.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: command.into(),
            args: Vec::new(),
        }
    }
}

impl ToArgs for WrapCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("wrap".into());
        self.global.render(args);
        args.push((&self.command).into());
        for arg in &self.args {
            args.push(arg.into());
        }
    }
}
