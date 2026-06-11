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
//! The [`EventAction`] sink trait lives alongside the runner that emits
//! into it ([`runner`]). The [`process`] module provides OS-level
//! process lifecycle, registry, and shutdown management used by the CLI
//! and compose layers. [`process_group`] owns a spawned process tree by its
//! OS process-group id so a cancel can SIGTERM/SIGKILL the whole tree; it is
//! a primitive, distinct from the run-record concepts under [`process`].
//!
//! Signal sources (triggers) live in the per-trigger CLI crates
//! (`iter-cron`, `iter-watch`, etc.); they connect to runners through the
//! Queue boundary contract (see [`queue`]).
//!
//! The runtime model is intentionally small: triggers produce signals,
//! queues carry them across a boundary, and runners apply each signal to an
//! agent inside a workspace.

#![warn(missing_docs)]
#![deny(rust_2018_idioms)]

pub mod agent;
pub mod home;
pub mod log;
pub mod process;
pub mod process_group;
pub mod prompt;
pub mod queue;
pub mod runner;
pub mod signal;
pub mod telemetry;
pub mod template;
pub mod workspace;

pub use agent::{Agent, AgentInvocation, AgentRun};
pub use prompt::{
    CmpOp, IterationField, Prompt, PromptGuard, PromptSelector, PromptTemplate, SelectorError,
};
pub use queue::{Priority, Queue};
pub use runner::{
    BoxError, BuilderError, ErrorSource, EventAction, EventDispatcher, EventName, HookEvent,
    IterationContext, IterationState, PreviousResult, Runner, RunnerBuilder, RunnerExitError,
    RunnerPolicy, RunnerSummary, RunnerTerminationReason, SharedSignal, SignalAcquisition,
};
pub use signal::{
    Metadata, MetadataError, MetadataKey, MetadataValue, Signal, SignalId, SignalKind,
};
pub use template::{
    IterationRenderContext, RunnerRenderContext, SignalContext, Template, TemplateError,
};
pub use workspace::{SandboxRequirements, Workspace, match_env_pattern};
