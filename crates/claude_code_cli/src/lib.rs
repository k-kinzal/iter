//! Type-safe command builder and executor for the Claude Code CLI.
//!
//! This crate targets Claude Code CLI `2.1.178`. The module layout mirrors the
//! CLI surface so version updates can be reviewed by command family: `execute`,
//! `agents`, `auth`, `mcp`, `plugin`, and so on.
#![doc = include_str!("../README.md")]

mod agents;
mod args;
mod auth;
mod auto_mode;
mod cli;
mod command;
mod execute;
mod install;
mod mcp;
mod output;
mod plugin;
mod project;
mod ultrareview;
mod values;

pub use agents::Agents;
pub use args::ToArgs;
pub use auth::{Auth, AuthHelpCommand, AuthLogin, AuthLoginProvider, AuthStatus, AuthStatusFormat};
pub use auto_mode::{AutoMode, AutoModeCritique, AutoModeHelpCommand};
pub use cli::{ClaudeCode, Error};
pub use command::{Doctor, SetupToken, Update};
pub use execute::{
    ExecuteCommand, JsonExecuteCommand, StreamJsonExecuteCommand, TextExecuteCommand,
};
pub use install::Install;
pub use mcp::{
    Mcp, McpAdd, McpAddFromClaudeDesktop, McpAddJson, McpHelpCommand, McpRemove, McpScope,
    McpServe, McpTransport,
};
pub use output::{
    JsonOutput, JsonOutputType, ResultSubtype, StreamEvent, StreamEventType, StreamOutput,
    TextOutput, parse_stream_json,
};
pub use plugin::{
    Plugin, PluginComponent, PluginDirectHelpCommand, PluginDisable, PluginDisableTarget,
    PluginEnable, PluginHelpCommand, PluginInit, PluginInstall, PluginList, PluginMarketplace,
    PluginMarketplaceAdd, PluginMarketplaceHelpCommand, PluginMarketplaceList,
    PluginMarketplaceRemove, PluginMarketplaceUpdate, PluginPrune, PluginScope, PluginTag,
    PluginUninstall, PluginUpdate, PluginValidate,
};
pub use project::{Project, ProjectHelpCommand, ProjectPurge, ProjectPurgeTarget};
pub use ultrareview::UltraReview;
pub use uuid;
pub use values::{
    BooleanChoice, Chrome, EffortLevel, FileResource, InputFormat, InvalidMaxBudgetUsd,
    MaxBudgetUsd, OptionalValue, PermissionMode, SettingSource, Switch, TmuxMode, ToolSet,
};

/// Claude Code CLI version this crate was authored against.
pub const SUPPORTED_CLAUDE_CODE_VERSION: &str = "2.1.178";

#[cfg(test)]
mod tests;
