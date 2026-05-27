//! Core runtime for iter: a [`Runner`] drives [`Signal`]s from a
//! [`Queue`] through a [`Workspace`] and an [`Agent`]. Ships standard
//! drivers for Queue and Agent.
//!
//! Each abstraction lives with the concept it represents: [`Agent`] in
//! [`agent`], [`Queue`] in [`queue`], [`Workspace`] in [`workspace`].
//! Queue and Agent implementations are drivers (`queue::drivers`,
//! `agent::drivers`), feature-gated for external dependencies.
//! Workspace implementations are core — not drivers.
//!
//! The [`EventHandler`] sink trait lives alongside the runner that emits
//! into it ([`runner`]). The [`process`] module provides OS-level
//! process lifecycle, registry, and shutdown management used by the CLI
//! and compose layers.
//!
//! Signal sources (triggers) live in `iter_trigger` and the per-trigger
//! CLI crates (`iter-cron`, `iter-watch`, etc.); they connect to
//! runners through the queue abstraction.
//!
//! The runtime model is intentionally small: triggers produce signals,
//! queues carry them across a boundary, and runners apply each signal to an
//! agent inside a workspace.

#![warn(missing_docs)]
#![deny(rust_2018_idioms)]

pub mod agent;
pub mod config;
pub mod error;
pub mod log;
pub mod process;
pub mod prompt;
pub mod queue;
pub mod runner;
pub mod signal;
pub mod telemetry;
pub mod template;
pub mod workspace;

pub use agent::{Agent, AgentReport, AgentRunContext, ExitStatus};
pub use config::{Config, ConfigError, LogLevel};
pub use error::{Error, Result};
pub use prompt::{
    CmpOp, IterationField, Prompt, PromptGuard, PromptSelector, PromptTemplate, SelectorError,
};
pub use queue::{Priority, Queue};
pub use runner::{
    BoxError, BuilderError, Event, EventEmitter, EventHandler, EventName,
    IterationContext,
    ShellEventHandler,
    IterationState, PreviousResult, Runner, RunnerBehavior, RunnerBuilder, RunnerConfig,
    RunnerExitError, RunnerSummary, RunnerTerminationReason,
};
pub use signal::{
    Metadata, MetadataError, MetadataKey, MetadataValue, Signal, SignalId, SignalKind,
};
pub use template::{LifecycleRenderContext, RenderContext, SignalContext, Template, TemplateError};
pub use workspace::{
    ITER_SANDBOX_COMMAND_PREFIX, SANDBOX_PREFIX_SEP, SandboxRequirements, Workspace,
    current_sandbox_prefix, decode_prefix_env, encode_prefix_env, match_env_pattern,
};
