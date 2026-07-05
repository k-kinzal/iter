//! Type-safe command builder and executor for the Grok CLI (`grok`).
//!
//! This crate targets Grok CLI `0.2.82`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run` (the
//! root interactive session), `single` (the headless `-p` run), `agent`,
//! `auth`, `mcp`, `memory`, `plugin`, `worktree`, `leader`, `session` (history:
//! `sessions` / `export` / `import` / `trace`), and `ops`.
//!
//! The [`output`] module models `grok -p … --output-format json` /
//! `streaming-json` output: the single terminal JSON object (or the streaming
//! `end` event that carries the same metadata) with typed accessors, plus a
//! lossless event stream. Deciding what a non-zero exit or a token-limit phrase
//! *means* is left to the caller.
#![doc = include_str!("../README.md")]

mod agent;
mod args;
mod auth;
mod cli;
mod leader;
mod mcp;
mod memory;
mod ops;
mod options;
mod output;
mod plugin;
mod run;
mod session;
mod single;
mod values;
mod worktree;

pub use agent::{AgentCommand, AgentTransport};
pub use args::ToArgs;
pub use auth::{LoginCommand, LogoutCommand};
pub use cli::{Error, ExecutableCommand, Grok};
pub use leader::{LeaderCommand, LeaderProfileSubcommand, LeaderSubcommand};
pub use mcp::{McpAdd, McpCommand, McpSubcommand};
pub use memory::{MemoryCommand, MemorySubcommand};
pub use ops::{
    CompletionsCommand, DashboardCommand, InspectCommand, ModelsCommand, SetupCommand,
    UpdateCommand, VersionCommand, WrapCommand,
};
pub use options::GlobalOptions;
pub use output::{Event, EventStream, EventType, SingleOutput, StopReason, Usage};
pub use plugin::{MarketplaceSubcommand, PluginCommand, PluginSubcommand};
pub use run::{RunCommand, RunOptions};
pub use session::{
    ExportCommand, ImportCommand, SessionsCommand, SessionsSubcommand, TraceCommand,
};
pub use single::{JsonSingleCommand, PromptSource, SingleCommand, StreamingSingleCommand};
pub use values::{
    CompletionShell, Effort, McpScope, McpTransport, OutputFormat, PermissionMode, ResumeTarget,
    Worktree,
};
pub use worktree::{WorktreeCommand, WorktreeDbSubcommand, WorktreeSubcommand};

/// Grok CLI version this crate was authored against.
pub const SUPPORTED_GROK_VERSION: &str = "0.2.82";

#[cfg(test)]
mod tests;
