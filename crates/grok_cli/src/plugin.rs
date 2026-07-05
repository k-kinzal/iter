//! `grok plugin` — manage plugins and plugin marketplaces.
//!
//! `grok plugin <COMMAND>` groups the plugin lifecycle
//! (`list`/`install`/`uninstall`/`update`/`enable`/`disable`/`details`/
//! `validate`/`tag`) plus the nested `marketplace` group.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag};
use crate::options::GlobalOptions;

/// A `grok plugin` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSubcommand {
    /// `list` — list installed plugins.
    List {
        /// `--json`: emit machine-readable JSON output.
        json: bool,
        /// `--available`: include plugins available from marketplaces.
        available: bool,
    },
    /// `install <SOURCE>` — install a plugin from a source.
    Install {
        /// Plugin source (marketplace id, path, or URL).
        source: String,
        /// `--trust`: trust the plugin without prompting.
        trust: bool,
    },
    /// `uninstall <NAME>` — remove an installed plugin (aliases `rm`,
    /// `remove`).
    Uninstall {
        /// Plugin name to remove.
        name: String,
        /// `--confirm`: skip the confirmation prompt.
        confirm: bool,
        /// `--keep-data`: keep the plugin's data directory.
        keep_data: bool,
    },
    /// `update [NAME]` — update one or all plugins.
    Update {
        /// Optional plugin name; updates all plugins when omitted.
        name: Option<String>,
    },
    /// `enable <NAME>` — enable an installed plugin.
    Enable {
        /// Plugin name to enable.
        name: String,
    },
    /// `disable <NAME>` — disable an installed plugin.
    Disable {
        /// Plugin name to disable.
        name: String,
    },
    /// `details <NAME>` — show details for a plugin.
    Details {
        /// Plugin name to inspect.
        name: String,
    },
    /// `validate [PATH]` — validate a plugin manifest (defaults to `.`).
    Validate {
        /// Optional plugin path; defaults to the current directory.
        path: Option<PathBuf>,
    },
    /// `tag [PATH]` — create and optionally push a plugin release tag
    /// (defaults to `.`).
    Tag {
        /// Optional plugin path; defaults to the current directory.
        path: Option<PathBuf>,
        /// `--push`: push the created tag to the remote.
        push: bool,
        /// `-f, --force`: overwrite an existing tag.
        force: bool,
        /// `--dry-run`: show what would happen without making changes.
        dry_run: bool,
    },
    /// `marketplace <COMMAND>` — manage plugin marketplaces.
    Marketplace(MarketplaceSubcommand),
}

/// A `grok plugin marketplace` subcommand.
///
/// The individual marketplace leaves do not expose their own `--help` in
/// `grok 0.2.82` (asking for it prints the root help), so their flag sets are
/// not verifiable from the CLI. They are modeled structurally; append any extra
/// flags through [`MarketplaceSubcommand::args`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceSubcommand {
    /// `list` — list configured marketplaces.
    List {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `add` — add a marketplace.
    Add {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `remove` — remove a marketplace.
    Remove {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `update` — update marketplace metadata.
    Update {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
}

impl MarketplaceSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("marketplace".into());
        let (leaf, extra) = match self {
            Self::List { args } => ("list", args),
            Self::Add { args } => ("add", args),
            Self::Remove { args } => ("remove", args),
            Self::Update { args } => ("update", args),
        };
        args.push(leaf.into());
        for arg in extra {
            args.push(arg.into());
        }
    }
}

impl PluginSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { json, available } => {
                args.push("list".into());
                push_flag(args, *json, "--json");
                push_flag(args, *available, "--available");
            }
            Self::Install { source, trust } => {
                args.push("install".into());
                push_flag(args, *trust, "--trust");
                args.push(source.into());
            }
            Self::Uninstall {
                name,
                confirm,
                keep_data,
            } => {
                args.push("uninstall".into());
                push_flag(args, *confirm, "--confirm");
                push_flag(args, *keep_data, "--keep-data");
                args.push(name.into());
            }
            Self::Update { name } => {
                args.push("update".into());
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::Enable { name } => {
                args.push("enable".into());
                args.push(name.into());
            }
            Self::Disable { name } => {
                args.push("disable".into());
                args.push(name.into());
            }
            Self::Details { name } => {
                args.push("details".into());
                args.push(name.into());
            }
            Self::Validate { path } => {
                args.push("validate".into());
                if let Some(path) = path {
                    args.push(path.into());
                }
            }
            Self::Tag {
                path,
                push,
                force,
                dry_run,
            } => {
                args.push("tag".into());
                push_flag(args, *push, "--push");
                push_flag(args, *force, "--force");
                push_flag(args, *dry_run, "--dry-run");
                if let Some(path) = path {
                    args.push(path.into());
                }
            }
            Self::Marketplace(marketplace) => marketplace.render(args),
        }
    }
}

/// `grok plugin [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `plugin` subcommand.
    pub command: PluginSubcommand,
}

impl PluginCommand {
    /// Build a `plugin` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: PluginSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
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
