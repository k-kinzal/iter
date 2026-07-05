//! Type-safe command builder and executor for the Cline CLI.
//!
//! This crate targets Cline CLI `3.0.23`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run` (the
//! root prompt run), `auth`, `plugin`, `history`, `schedule`, `hub`,
//! `connect`, and the small management commands in `ops` (`config`, `doctor`,
//! `mcp`, `hook`, `dashboard`, `update`, `version`, `kanban`).
//!
//! The [`output`] module models `cline --json`'s NDJSON run stream as a
//! lossless event log with typed accessors for the terminal verdict. Deciding
//! what a non-zero exit or a usage limit *means* is left to the caller.
#![doc = include_str!("../README.md")]

mod args;
mod auth;
mod cli;
mod connect;
mod history;
mod hub;
mod ops;
mod output;
mod plugin;
mod run;
mod schedule;
mod values;

pub use args::ToArgs;
pub use auth::AuthCommand;
pub use cli::{Cline, Error};
pub use connect::ConnectCommand;
pub use history::{HistoryCommand, HistorySubcommand};
pub use hub::{HubCommand, HubSubcommand};
pub use ops::{
    ConfigCommand, DashboardCommand, DoctorCommand, DoctorSubcommand, HookCommand, KanbanCommand,
    McpCommand, UpdateCommand, VersionCommand,
};
pub use output::{Event, EventStream, EventType, FinishReason, RunOutput, RunVerdict};
pub use plugin::{PluginCommand, PluginSubcommand};
pub use run::{JsonRunCommand, RunCommand, RunOptions};
pub use schedule::{ScheduleCommand, ScheduleCreateOptions, ScheduleSubcommand};
pub use values::{AgentMode, CompactionMode, ThinkingLevel};

/// Cline CLI version this crate was authored against.
pub const SUPPORTED_CLINE_VERSION: &str = "3.0.23";

#[cfg(test)]
mod tests;
