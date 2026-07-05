//! Type-safe command builder and executor for the opencode CLI.
//!
//! This crate targets opencode `1.2.20`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run` (the
//! headless run command) and the root interactive TUI, the server commands
//! (`acp` / `serve` / `web`), the management trees (`session`, `auth`,
//! `agent`, `mcp`, `github`, `db`, `debug`), and the operational commands
//! (`completion`, `attach`, `upgrade`, `uninstall`, `models`, `stats`,
//! `export`, `import`, `pr`).
//!
//! The [`output`] module models `opencode run --format json`'s event stream as
//! a lossless event log with typed accessors for the terminal verdict. opencode
//! is one of the exit-0-but-failed CLIs: deciding what an error event or a
//! usage limit *means* is left to the caller.
#![doc = include_str!("../README.md")]

mod agent;
mod args;
mod auth;
mod cli;
mod db;
mod debug;
mod github;
mod mcp;
mod ops;
mod options;
mod output;
mod run;
mod server;
mod session;
mod values;

pub use agent::{AgentCommand, AgentSubcommand};
pub use args::ToArgs;
pub use auth::{AuthCommand, AuthSubcommand};
pub use cli::{Error, Opencode};
pub use db::{DbCommand, DbSubcommand};
pub use debug::{
    DebugCommand, DebugFileSubcommand, DebugLspSubcommand, DebugRgSubcommand,
    DebugSnapshotSubcommand, DebugSubcommand,
};
pub use github::{GithubCommand, GithubSubcommand};
pub use mcp::{McpAuthSubcommand, McpCommand, McpSubcommand};
pub use ops::{
    AttachCommand, CompletionCommand, ExportCommand, ImportCommand, ModelsCommand, PrCommand,
    StatsCommand, UninstallCommand, UpgradeCommand,
};
pub use options::{GlobalOptions, ServerOptions};
pub use output::{Event, EventStream, EventType, RunError, RunOutput};
pub use run::{JsonRunCommand, RunCommand, RunOptions, TuiCommand};
pub use server::{AcpCommand, ServeCommand, WebCommand};
pub use session::{SessionCommand, SessionSubcommand};
pub use values::{
    AgentMode, Continuation, DbFormat, LogLevel, OutputFormat, SessionFormat, StatsModels,
    UpgradeMethod,
};

/// opencode CLI version this crate was authored against.
pub const SUPPORTED_OPENCODE_VERSION: &str = "1.2.20";

#[cfg(test)]
mod tests;
