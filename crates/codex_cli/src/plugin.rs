//! `codex plugin` — manage Codex plugins and their marketplaces.
//!
//! Each leaf (`add`, `list`, `remove`) and each `marketplace` leaf (`add`,
//! `list`, `upgrade`, `remove`) has its own `--help`, so the shapes are modeled
//! with typed positionals and flags rather than a raw-args passthrough. The
//! config family (`-c/--config`, `--enable`, `--disable`) is accepted at the
//! `plugin` parent level and rendered via [`GlobalConfig`].

use std::ffi::OsString;

use crate::args::{ToArgs, push_each, push_flag, push_opt};
use crate::options::GlobalConfig;

/// `codex plugin [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// The plugin subcommand.
    pub command: PluginSubcommand,
}

impl PluginCommand {
    /// Wrap a plugin subcommand with default global options.
    #[must_use]
    pub fn new(command: PluginSubcommand) -> Self {
        Self {
            global: GlobalConfig::default(),
            command,
        }
    }
}

impl ToArgs for PluginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("plugin".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// A `codex plugin` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSubcommand {
    /// `plugin add [OPTIONS] <PLUGIN[@MARKETPLACE]>`.
    Add {
        /// Plugin selector: `PLUGIN@MARKETPLACE` or `PLUGIN` with `--marketplace`.
        plugin: String,
        /// `-m, --marketplace <MARKETPLACE>`.
        marketplace: Option<String>,
        /// `--json`: output the install result as JSON.
        json: bool,
    },
    /// `plugin list [OPTIONS]`.
    List {
        /// `-m, --marketplace <MARKETPLACE>`: only list plugins from this
        /// marketplace.
        marketplace: Option<String>,
        /// `--json`: output the plugin list as JSON.
        json: bool,
        /// `--available`: include uninstalled marketplace plugins in the JSON
        /// output.
        available: bool,
    },
    /// `plugin remove [OPTIONS] <PLUGIN[@MARKETPLACE]>`.
    Remove {
        /// Plugin selector: `PLUGIN@MARKETPLACE` or `PLUGIN` with `--marketplace`.
        plugin: String,
        /// `-m, --marketplace <MARKETPLACE>`.
        marketplace: Option<String>,
        /// `--json`: output the remove result as JSON.
        json: bool,
    },
    /// `plugin marketplace <COMMAND>`.
    Marketplace(PluginMarketplaceSubcommand),
}

impl PluginSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add {
                plugin,
                marketplace,
                json,
            } => {
                args.push("add".into());
                push_opt(args, "--marketplace", marketplace.as_deref());
                push_flag(args, *json, "--json");
                args.push(plugin.into());
            }
            Self::List {
                marketplace,
                json,
                available,
            } => {
                args.push("list".into());
                push_opt(args, "--marketplace", marketplace.as_deref());
                push_flag(args, *json, "--json");
                push_flag(args, *available, "--available");
            }
            Self::Remove {
                plugin,
                marketplace,
                json,
            } => {
                args.push("remove".into());
                push_opt(args, "--marketplace", marketplace.as_deref());
                push_flag(args, *json, "--json");
                args.push(plugin.into());
            }
            Self::Marketplace(command) => {
                args.push("marketplace".into());
                command.render(args);
            }
        }
    }
}

/// A `codex plugin marketplace` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginMarketplaceSubcommand {
    /// `plugin marketplace add [OPTIONS] <SOURCE>`.
    Add {
        /// Marketplace source: local path, `owner/repo[@ref]`, or Git URL.
        source: String,
        /// `--ref <REF>`: Git ref to fetch for Git marketplace sources.
        git_ref: Option<String>,
        /// `--sparse <PATH>` (repeatable): sparse-checkout path for Git sources.
        sparse: Vec<String>,
        /// `--json`: output the add result as JSON.
        json: bool,
    },
    /// `plugin marketplace list [--json]`.
    List {
        /// `--json`: output the marketplace list as JSON.
        json: bool,
    },
    /// `plugin marketplace upgrade [--json] [MARKETPLACE_NAME]`.
    Upgrade {
        /// Optional configured marketplace name; omit to upgrade all.
        name: Option<String>,
        /// `--json`: output the upgrade result as JSON.
        json: bool,
    },
    /// `plugin marketplace remove [--json] <MARKETPLACE_NAME>`.
    Remove {
        /// Configured marketplace name to remove.
        name: String,
        /// `--json`: output the remove result as JSON.
        json: bool,
    },
}

impl PluginMarketplaceSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add {
                source,
                git_ref,
                sparse,
                json,
            } => {
                args.push("add".into());
                push_opt(args, "--ref", git_ref.as_deref());
                push_each(args, "--sparse", sparse);
                push_flag(args, *json, "--json");
                args.push(source.into());
            }
            Self::List { json } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
            }
            Self::Upgrade { name, json } => {
                args.push("upgrade".into());
                push_flag(args, *json, "--json");
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::Remove { name, json } => {
                args.push("remove".into());
                push_flag(args, *json, "--json");
                args.push(name.into());
            }
        }
    }
}
