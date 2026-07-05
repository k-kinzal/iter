//! Type-safe command builders and executor for Nous Research's Hermes Agent
//! CLI (`hermes`).
//!
//! This crate targets Hermes `0.16.0`. The module layout mirrors the CLI
//! surface so version updates can be reviewed by command family: the root
//! [`run`], [`chat`], [`send`], [`sessions`], [`mcp`], [`auth`], and the
//! remaining operator [`ops`].
//!
//! # The agent path is text-only
//!
//! Hermes runs the model two ways — `hermes -z <PROMPT>` (oneshot) and
//! `hermes chat -Q` (quiet) — and **both are plain text**. A run's "output" is
//! its decoded stdout/stderr plus a typed [`Exit`] classification
//! ([`output`]); there is no per-turn event stream to parse.
//!
//! # The two JSON surfaces
//!
//! Only [`send --json`](send::SendOutput) and
//! [`sessions export`](sessions::SessionExport) emit structured output, and
//! neither runs the agent. Both parsers preserve each JSON payload losslessly
//! and expose typed accessors, leaving what a non-zero exit or a delivery
//! failure *means* to the caller.
#![doc = include_str!("../README.md")]

mod args;
mod auth;
mod chat;
mod cli;
mod mcp;
mod ops;
mod output;
mod run;
mod send;
mod sessions;
mod values;

pub use args::ToArgs;
pub use auth::{
    AuthAddOptions, AuthCommand, AuthSubcommand, LoginCommand, LogoutCommand, NousOauthOptions,
};
pub use chat::{ChatCommand, ChatOptions};
pub use cli::{Error, Hermes};
pub use mcp::{McpAddOptions, McpCommand, McpSubcommand, McpTransport};
pub use ops::{
    CompletionCommand, ConfigCommand, ConfigSubcommand, ModelCommand, RawCommand, StatusCommand,
    ToolsCommand, ToolsSubcommand, UpdateCommand, VersionCommand,
};
pub use output::{Exit, RunOutput};
pub use run::{ContinueMode, RunCommand, RunMode, RunOptions};
pub use send::{SendCommand, SendOutput};
pub use sessions::{ExportRecord, SessionExport, SessionsCommand, SessionsSubcommand};
pub use values::{CredentialType, LoginProvider, LogoutProvider, McpAuth, Shell, SpotifyAction};

/// Hermes CLI version this crate was authored against.
pub const SUPPORTED_HERMES_VERSION: &str = "0.16.0";

#[cfg(test)]
mod tests;
