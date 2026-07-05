//! `agy plugin <command>` — plugin management.
//!
//! `plugins` is an alias for `plugin`; the canonical `plugin` form is modeled.
//! The leaf subcommands take positional operands and expose no `--help` of
//! their own.

use std::ffi::OsString;

use crate::args::ToArgs;
use crate::values::ImportSource;

/// A `agy plugin` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSubcommand {
    /// `list`: list imported plugins.
    List,
    /// `import [source]`: import plugins from `gemini` or `claude`.
    Import {
        /// Optional source (`gemini` / `claude`).
        source: Option<ImportSource>,
    },
    /// `install <target>`: install a plugin (supports `plugin@marketplace`).
    Install {
        /// Install target.
        target: String,
    },
    /// `uninstall <name>`: uninstall a plugin.
    Uninstall {
        /// Plugin name.
        name: String,
    },
    /// `enable <name>`: enable a plugin.
    Enable {
        /// Plugin name.
        name: String,
    },
    /// `disable <name>`: disable a plugin.
    Disable {
        /// Plugin name.
        name: String,
    },
    /// `validate [path]`: validate a plugin.
    Validate {
        /// Optional plugin path.
        path: Option<String>,
    },
    /// `link <mp> <target>`: generate a link to a marketplace.
    Link {
        /// Marketplace.
        marketplace: String,
        /// Link target.
        target: String,
    },
    /// `help`: show plugin help.
    Help,
}

impl PluginSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List => args.push("list".into()),
            Self::Import { source } => {
                args.push("import".into());
                if let Some(source) = source {
                    args.push(source.as_str().into());
                }
            }
            Self::Install { target } => {
                args.push("install".into());
                args.push(target.into());
            }
            Self::Uninstall { name } => {
                args.push("uninstall".into());
                args.push(name.into());
            }
            Self::Enable { name } => {
                args.push("enable".into());
                args.push(name.into());
            }
            Self::Disable { name } => {
                args.push("disable".into());
                args.push(name.into());
            }
            Self::Validate { path } => {
                args.push("validate".into());
                if let Some(path) = path {
                    args.push(path.into());
                }
            }
            Self::Link {
                marketplace,
                target,
            } => {
                args.push("link".into());
                args.push(marketplace.into());
                args.push(target.into());
            }
            Self::Help => args.push("help".into()),
        }
    }
}

/// `agy plugin <command>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// The leaf subcommand.
    pub subcommand: PluginSubcommand,
}

impl PluginCommand {
    /// Wrap a [`PluginSubcommand`].
    #[must_use]
    pub fn new(subcommand: PluginSubcommand) -> Self {
        Self { subcommand }
    }

    /// `agy plugin list`.
    #[must_use]
    pub fn list() -> Self {
        Self::new(PluginSubcommand::List)
    }

    /// `agy plugin install <target>`.
    #[must_use]
    pub fn install(target: impl Into<String>) -> Self {
        Self::new(PluginSubcommand::Install {
            target: target.into(),
        })
    }

    /// `agy plugin uninstall <name>`.
    #[must_use]
    pub fn uninstall(name: impl Into<String>) -> Self {
        Self::new(PluginSubcommand::Uninstall { name: name.into() })
    }
}

impl ToArgs for PluginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("plugin".into());
        self.subcommand.render(args);
    }
}
