//! The remaining operator-facing command families — `config`, `tools`,
//! `model`, `status`, `version`, `update`, `completion` — and [`RawCommand`],
//! the escape hatch for Hermes' broader subcommand tree.

use std::ffi::OsString;

use crate::args::{
    ToArgs, push_flag, push_opt, push_opt_positional, push_positional, push_positionals,
};
use crate::auth::NousOauthOptions;
use crate::values::Shell;

/// A `hermes config` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigSubcommand {
    /// `show`: show current configuration.
    Show,
    /// `edit`: open the config file in `$EDITOR`.
    Edit,
    /// `set [KEY] [VALUE]`: set a configuration value.
    ///
    /// The positionals are order-sensitive, so a value can never be supplied
    /// without its key: the whole pair is optional, and the value within it is
    /// optional, but a lone value is unrepresentable.
    Set {
        /// The `(KEY, VALUE)` pair. `None` targets bare `config set`;
        /// `Some((key, None))` sets a key with no value; `Some((key,
        /// Some(value)))` sets both.
        pair: Option<(String, Option<String>)>,
    },
    /// `path`: print the config file path.
    Path,
    /// `env-path`: print the `.env` file path.
    EnvPath,
    /// `check`: check for missing/outdated config.
    Check,
    /// `migrate`: update config with new options.
    Migrate,
}

impl ConfigSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Show => args.push("show".into()),
            Self::Edit => args.push("edit".into()),
            Self::Set { pair } => {
                args.push("set".into());
                if let Some((key, value)) = pair {
                    push_positional(args, key);
                    push_opt_positional(args, value.as_deref());
                }
            }
            Self::Path => args.push("path".into()),
            Self::EnvPath => args.push("env-path".into()),
            Self::Check => args.push("check".into()),
            Self::Migrate => args.push("migrate".into()),
        }
    }
}

/// `hermes config <command>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCommand {
    /// The leaf subcommand.
    pub subcommand: ConfigSubcommand,
}

impl ConfigCommand {
    /// Wrap a [`ConfigSubcommand`].
    #[must_use]
    pub fn new(subcommand: ConfigSubcommand) -> Self {
        Self { subcommand }
    }

    /// `hermes config show`.
    #[must_use]
    pub fn show() -> Self {
        Self::new(ConfigSubcommand::Show)
    }

    /// `hermes config set <KEY> <VALUE>`.
    #[must_use]
    pub fn set(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(ConfigSubcommand::Set {
            pair: Some((key.into(), Some(value.into()))),
        })
    }
}

impl ToArgs for ConfigCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("config".into());
        self.subcommand.render(args);
    }
}

/// A `hermes tools` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolsSubcommand {
    /// `list`: show all tools and their enabled/disabled status.
    List {
        /// `--platform <PLATFORM>`: platform to show (default `cli`).
        platform: Option<String>,
    },
    /// `enable <NAME...>`: enable toolsets or MCP tools.
    Enable {
        /// `--platform <PLATFORM>`: platform to apply to (default `cli`).
        platform: Option<String>,
        /// Toolset names or MCP tools in `server:tool` form.
        names: Vec<String>,
    },
    /// `disable <NAME...>`: disable toolsets or MCP tools.
    Disable {
        /// `--platform <PLATFORM>`: platform to apply to (default `cli`).
        platform: Option<String>,
        /// Toolset names or MCP tools in `server:tool` form.
        names: Vec<String>,
    },
    /// `post-setup <KEY>`: run a provider's post-setup install hook.
    PostSetup {
        /// The post-setup hook key (e.g. `agent_browser`, `camofox`).
        key: String,
    },
}

impl ToolsSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { platform } => {
                args.push("list".into());
                push_opt(args, "--platform", platform.as_deref());
            }
            Self::Enable { platform, names } => {
                args.push("enable".into());
                push_opt(args, "--platform", platform.as_deref());
                push_positionals(args, names);
            }
            Self::Disable { platform, names } => {
                args.push("disable".into());
                push_opt(args, "--platform", platform.as_deref());
                push_positionals(args, names);
            }
            Self::PostSetup { key } => {
                args.push("post-setup".into());
                push_positional(args, key);
            }
        }
    }
}

/// `hermes tools [--summary] [<command>]`.
///
/// With no subcommand and no `--summary`, the CLI opens its interactive
/// configuration UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolsCommand {
    /// `--summary`: print a summary of enabled tools per platform and exit.
    pub summary: bool,
    /// The optional leaf subcommand.
    pub subcommand: Option<ToolsSubcommand>,
}

impl ToolsCommand {
    /// `hermes tools list`.
    #[must_use]
    pub fn list() -> Self {
        Self {
            summary: false,
            subcommand: Some(ToolsSubcommand::List { platform: None }),
        }
    }
}

impl ToArgs for ToolsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("tools".into());
        push_flag(args, self.summary, "--summary");
        if let Some(subcommand) = &self.subcommand {
            subcommand.render(args);
        }
    }
}

/// `hermes model` — interactively select the inference provider and default
/// model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCommand {
    /// `--refresh`: wipe the model picker disk cache and re-fetch live model
    /// lists.
    pub refresh: bool,
    /// `--manual-paste`: skip the loopback callback listener for OAuth
    /// providers.
    pub manual_paste: bool,
    /// The shared Nous OAuth option block.
    pub oauth: NousOauthOptions,
}

impl ToArgs for ModelCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("model".into());
        push_flag(args, self.refresh, "--refresh");
        push_flag(args, self.manual_paste, "--manual-paste");
        self.oauth.render(args);
    }
}

/// `hermes status` — display the status of Hermes Agent components.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusCommand {
    /// `--all`: show all details (redacted for sharing).
    pub all: bool,
    /// `--deep`: run deep checks (may take longer).
    pub deep: bool,
}

impl ToArgs for StatusCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("status".into());
        push_flag(args, self.all, "--all");
        push_flag(args, self.deep, "--deep");
    }
}

/// `hermes version` — show version information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionCommand;

impl ToArgs for VersionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("version".into());
    }
}

/// `hermes update` — pull the latest changes and reinstall dependencies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand {
    /// `--gateway`: use file-based IPC for prompts (internal).
    pub gateway: bool,
    /// `--check`: check whether an update is available without installing.
    pub check: bool,
    /// `--no-backup`: skip the pre-update backup for this run.
    pub no_backup: bool,
    /// `--backup`: force a pre-update backup for this run.
    pub backup: bool,
    /// `--yes`: assume yes for interactive prompts.
    pub yes: bool,
    /// `--branch <NAME>`: update against this branch instead of the default.
    pub branch: Option<String>,
    /// `--force`: Windows — proceed even when another `hermes.exe` is detected.
    pub force: bool,
}

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
        push_flag(args, self.gateway, "--gateway");
        push_flag(args, self.check, "--check");
        push_flag(args, self.no_backup, "--no-backup");
        push_flag(args, self.backup, "--backup");
        push_flag(args, self.yes, "--yes");
        push_opt(args, "--branch", self.branch.as_deref());
        push_flag(args, self.force, "--force");
    }
}

/// `hermes completion [SHELL]` — print a shell completion script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionCommand {
    /// The shell to emit completions for (the CLI defaults to `bash`).
    pub shell: Option<Shell>,
}

impl CompletionCommand {
    /// `hermes completion <SHELL>`.
    #[must_use]
    pub fn shell(shell: Shell) -> Self {
        Self { shell: Some(shell) }
    }
}

impl ToArgs for CompletionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("completion".into());
        if let Some(shell) = self.shell {
            push_positional(args, shell.as_str());
        }
    }
}

/// A typed escape hatch for any Hermes subcommand not modeled with its own
/// builder (gateway, proxy, doctor, skills, …).
///
/// It renders the subcommand `name` followed by verbatim `args`, so callers can
/// still construct an arbitrary invocation through the same [`Hermes`]
/// executor.
///
/// [`Hermes`]: crate::Hermes
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawCommand {
    /// The subcommand name (e.g. `doctor`).
    pub name: String,
    /// Verbatim arguments appended after the subcommand name.
    pub args: Vec<String>,
}

impl RawCommand {
    /// A raw subcommand with no arguments.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
        }
    }

    /// A raw subcommand with verbatim arguments.
    #[must_use]
    pub fn with_args(name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }
}

impl ToArgs for RawCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        push_positional(args, &self.name);
        push_positionals(args, &self.args);
    }
}
