//! `copilot plugin` — manage plugins and plugin marketplaces.
//!
//! The `plugin` root takes no options of its own. The `install`/`list`/
//! `uninstall`/`update` leaves are modeled with their verified shapes; the
//! nested `marketplace` tree (`add`/`browse`/`list`/`remove`/`update`) is
//! carried as a raw-args passthrough.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};

/// `copilot plugin <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// The plugin subcommand.
    pub command: PluginSubcommand,
}

impl PluginCommand {
    /// Wrap a plugin subcommand.
    #[must_use]
    pub fn new(command: PluginSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for PluginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("plugin".into());
        self.command.render(args);
    }
}

/// A `copilot plugin` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSubcommand {
    /// `plugin install <SOURCE>`: `plugin@marketplace`, `owner/repo`,
    /// `owner/repo:path`, or a git URL.
    Install {
        /// The plugin source.
        source: String,
    },
    /// `plugin list`.
    List,
    /// `plugin uninstall <NAME>`.
    Uninstall {
        /// Plugin name (`plugin-name` or `plugin-name@marketplace-name`).
        name: String,
    },
    /// `plugin update [--all] [NAME]`.
    Update {
        /// `--all`: update every installed plugin.
        all: bool,
        /// Plugin name; omit when using `--all`.
        name: Option<String>,
    },
    /// `plugin marketplace <COMMAND> [ARGS]...`.
    Marketplace {
        /// Marketplace subcommand (`add`/`browse`/`list`/`remove`/`update`)
        /// and its arguments.
        args: Vec<String>,
    },
}

impl PluginSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Install { source } => {
                args.push("install".into());
                args.push(source.into());
            }
            Self::List => args.push("list".into()),
            Self::Uninstall { name } => {
                args.push("uninstall".into());
                args.push(name.into());
            }
            Self::Update { all, name } => {
                args.push("update".into());
                push_flag(args, *all, "--all");
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::Marketplace { args: rest } => {
                args.push("marketplace".into());
                args.extend(rest.iter().map(OsString::from));
            }
        }
    }
}
