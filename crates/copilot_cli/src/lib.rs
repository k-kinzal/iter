//! Type-safe command builder and executor for the GitHub Copilot CLI.
//!
//! This crate targets Copilot CLI `1.0.49`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run` (the
//! root run — interactive or one-shot `-p/--prompt`), `mcp`, `plugin`, and the
//! operational leaves (`completion`, `login`, `update`, `version`, `init`,
//! `help`).
//!
//! The [`output`] module models `copilot --output-format json`'s JSONL event
//! stream as a lossless event log with typed accessors for the two terminal
//! records ([`ResultRecord`] and [`SessionError`]). Deciding what a
//! `session.error` or a non-zero exit *means* is left to the caller.
//!
//! There is no `suggest` subcommand: the root `copilot` command *is* the run.
#![doc = include_str!("../README.md")]

mod args;
mod cli;
mod mcp;
mod ops;
mod output;
mod plugin;
mod run;
mod values;

pub use args::ToArgs;
pub use cli::{Copilot, Error};
pub use mcp::{McpAddOptions, McpCommand, McpSubcommand, McpTransport};
pub use ops::{
    CompletionCommand, HelpCommand, InitCommand, LoginCommand, UpdateCommand, VersionCommand,
};
pub use output::{Event, EventStream, EventType, ResultRecord, RunOutput, SessionError, Usage};
pub use plugin::{PluginCommand, PluginSubcommand};
pub use run::{JsonRunCommand, RunCommand, RunOptions, ShareTarget};
pub use values::{
    LogLevel, Mode, OutputFormat, ReasoningEffort, SessionSelector, Shell, Toggle, UpdateChannel,
};

/// GitHub Copilot CLI version this crate was authored against.
pub const SUPPORTED_COPILOT_VERSION: &str = "1.0.49";

#[cfg(test)]
mod tests;
