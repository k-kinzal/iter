//! `codex features` — inspect and toggle feature flags in `config.toml`.

use std::ffi::OsString;

use crate::args::ToArgs;
use crate::options::GlobalConfig;

/// `codex features [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeaturesCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// The features subcommand.
    pub command: FeaturesSubcommand,
}

impl FeaturesCommand {
    /// Wrap a features subcommand with default global options.
    #[must_use]
    pub fn new(command: FeaturesSubcommand) -> Self {
        Self {
            global: GlobalConfig::default(),
            command,
        }
    }
}

impl ToArgs for FeaturesCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("features".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// A `codex features` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FeaturesSubcommand {
    /// `features list`: list known features with their stage and state.
    List,
    /// `features enable <NAME>`.
    Enable {
        /// Feature name.
        name: String,
    },
    /// `features disable <NAME>`.
    Disable {
        /// Feature name.
        name: String,
    },
}

impl FeaturesSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List => args.push("list".into()),
            Self::Enable { name } => {
                args.push("enable".into());
                args.push(name.into());
            }
            Self::Disable { name } => {
                args.push("disable".into());
                args.push(name.into());
            }
        }
    }
}
