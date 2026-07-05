//! Type-safe command builders and executor for the Cursor Agent CLI
//! (`cursor-agent`).
//!
//! This crate targets `cursor-agent` `2026.03.11-6dfa30c`. The module layout
//! mirrors the CLI surface so version updates can be reviewed by command
//! family: `run` (the root agent run and its `--print` headless mode), `auth`
//! (`login` / `logout` / `status`), `mcp`, `session` (`create-chat` / `ls` /
//! `resume`), and `ops` (`models` / `about` / `update` / `generate-rule` /
//! shell integration).
//!
//! The [`output`] module models `cursor-agent --print`'s `json` and
//! `stream-json` output as a record log with typed accessors for the terminal
//! `result` verdict. Deciding what a non-zero exit or a missing terminal record
//! *means* is left to the caller.
#![doc = include_str!("../README.md")]

mod args;
mod auth;
mod cli;
mod mcp;
mod ops;
mod output;
mod run;
mod session;
mod values;

pub use args::ToArgs;
pub use auth::{LoginCommand, LogoutCommand, StatusCommand};
pub use cli::{Cursor, Error};
pub use mcp::{McpCommand, McpSubcommand};
pub use ops::{
    AboutCommand, GenerateRuleCommand, InstallShellIntegrationCommand, ModelsCommand,
    UninstallShellIntegrationCommand, UpdateCommand,
};
pub use output::{Event, EventStream, EventType, PrintOutput, Usage};
pub use run::{AgentCommand, PrintCommand, RunCommand, RunOptions};
pub use session::{CreateChatCommand, LsCommand, ResumeCommand};
pub use values::{ExecutionMode, OutputFormat, ResumeSelector, SandboxMode, Worktree};

/// `cursor-agent` version this crate was authored against.
pub const SUPPORTED_CURSOR_VERSION: &str = "2026.03.11-6dfa30c";

#[cfg(test)]
mod tests;
