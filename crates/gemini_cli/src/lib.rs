//! Type-safe command builder and executor for Google's Gemini CLI (`gemini`).
//!
//! This crate targets Gemini CLI `0.41.2`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: `run` (the
//! root interactive / headless run), and the management subcommand tree —
//! `mcp`, `extensions`, `skills`, `hooks`, and `gemma`.
//!
//! The [`output`] module models Gemini's two machine-readable modes: the single
//! `-o json` terminal record ([`GeminiOutput`]) and the `-o stream-json`
//! newline-delimited event stream ([`StreamOutput`] / [`StreamEvent`]).
//! Deciding what a non-zero exit, an `error` field, or a usage limit *means* is
//! left to the caller.
#![doc = include_str!("../README.md")]

mod args;
mod cli;
mod extensions;
mod gemma;
mod hooks;
mod mcp;
mod output;
mod run;
mod skills;
mod values;

pub use args::ToArgs;
pub use cli::{Error, Gemini};
pub use extensions::{ExtensionsCommand, ExtensionsSubcommand};
pub use gemma::{GemmaCommand, GemmaSubcommand};
pub use hooks::{HooksCommand, HooksSubcommand};
pub use mcp::{McpCommand, McpSubcommand};
pub use output::{
    EventStream, GeminiOutput, ResultError, StreamEvent, StreamEventType, StreamOutput, TokenStats,
};
pub use run::{JsonRunCommand, RunCommand, RunOptions, StreamRunCommand};
pub use skills::{SkillsCommand, SkillsSubcommand};
pub use values::{
    ApprovalMode, ExtensionTemplate, ExtensionsOutputFormat, McpScope, McpTransport, OutputFormat,
    Scope, SessionRef, Worktree,
};

/// Gemini CLI version this crate was authored against.
pub const SUPPORTED_GEMINI_VERSION: &str = "0.41.2";

#[cfg(test)]
mod tests;
