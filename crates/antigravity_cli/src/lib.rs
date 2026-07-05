//! Type-safe command builder and executor for Google's Antigravity CLI (`agy`).
//!
//! This crate targets Antigravity `1.0.16`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run`,
//! `plugin`, and the remaining `ops` subcommands.
#![doc = include_str!("../README.md")]

mod args;
mod cli;
mod ops;
mod output;
mod plugin;
mod run;
mod values;

pub use args::ToArgs;
pub use cli::{Antigravity, Error};
pub use ops::{ChangelogCommand, HelpCommand, InstallCommand, ModelsCommand, UpdateCommand};
pub use output::{Exit, RunOutput};
pub use plugin::{PluginCommand, PluginSubcommand};
pub use run::{RunCommand, RunMode, RunOptions};
pub use values::{GoDuration, ImportSource};

/// Antigravity CLI version this crate was authored against.
pub const SUPPORTED_ANTIGRAVITY_VERSION: &str = "1.0.16";

#[cfg(test)]
mod tests;
