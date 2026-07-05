//! Type-safe command builder and executor for the `OpenAI` Codex CLI.
//!
//! This crate targets Codex CLI `0.139.0`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run`
//! (the root interactive session), `exec`, `review`, `session` (resume / fork
//! / archive), `auth`, `mcp`, `plugin`, `features`, and `ops`.
//!
//! The [`output`] module models `codex exec --json`'s JSONL event stream as a
//! lossless event log with typed accessors for the terminal verdict. Deciding
//! what a non-zero exit or a usage limit *means* is left to the caller.
#![doc = include_str!("../README.md")]

mod args;
mod auth;
mod cli;
mod exec;
mod features;
mod mcp;
mod ops;
mod options;
mod output;
mod plugin;
mod review;
mod run;
mod session;
mod values;

pub use args::ToArgs;
pub use auth::{LoginCommand, LogoutCommand};
pub use cli::{Codex, Error};
pub use exec::{
    ExecCommand, ExecOptions, ExecResumeCommand, ExecReviewCommand, ExecSubcommandOptions,
    JsonExecCommand,
};
pub use features::{FeaturesCommand, FeaturesSubcommand};
pub use mcp::{McpCommand, McpSubcommand, McpTransport};
pub use ops::{
    AppCommand, AppServerCommand, ApplyCommand, CloudCommand, CompletionCommand, DebugCommand,
    DebugSubcommand, DoctorCommand, ExecServerCommand, McpServerCommand, RemoteControlCommand,
    RemoteControlSubcommand, SandboxCommand, UpdateCommand,
};
pub use options::{CommonConfig, GlobalConfig};
pub use output::{Event, EventStream, EventType, ExecOutput, TurnStatus, TurnVerdict};
pub use plugin::{PluginCommand, PluginMarketplaceSubcommand, PluginSubcommand};
pub use review::ReviewCommand;
pub use run::{RunCommand, RunOptions};
pub use session::{ArchiveCommand, ForkCommand, ResumeCommand, UnarchiveCommand};
pub use values::{
    ApprovalPolicy, Color, CompletionShell, ConfigOverride, LocalProvider, SandboxMode,
};

/// Codex CLI version this crate was authored against.
pub const SUPPORTED_CODEX_VERSION: &str = "0.139.0";

#[cfg(test)]
mod tests;
