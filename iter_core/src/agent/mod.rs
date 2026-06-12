//! Agent trait, shared primitives, and driver implementations.
//!
//! The [`Agent`] trait and associated types live at this level. Concrete
//! implementations live under [`drivers`]. Shared OS-level primitives
//! (subprocess management, hook lifecycle) are
//! `pub(crate)` internal modules used by the drivers.
//!
//! This module provides fourteen concrete implementations of the
//! [`Agent`] trait. Thirteen are CLI-backed drivers, falling into three
//! broad groups based on how the underlying CLI is driven; the fourteenth,
//! [`AgentRouter`], is a composite that drives no CLI of its own (see the
//! **Composite** group below).
//!
//! * **Hook-capable** — [`ClaudeAgent`], [`CodexAgent`], [`GeminiAgent`],
//!   [`HermesAgent`] (hook integration pending), [`AntigravityAgent`],
//!   and [`CopilotAgent`] each run in either
//!   [`AgentMode::Headless`]
//!   (non-interactive one-shot invocation whose machine-readable output is
//!   parsed by the per-CLI Command) or
//!   [`AgentMode::Interactive`] (live TUI session driven by a
//!   project-local Stop-style hook installed under the agent's
//!   own config directory).
//! * **Print-only** — [`CursorAgent`], [`ClineAgent`], [`OpenCodeAgent`],
//!   [`GrokAgent`], and [`GenericAgent`]. These tools run to completion on
//!   every invocation with no hook installation. [`GrokAgent`] additionally
//!   persists a session id (Grok Build's `-s/--session-id`) for
//!   continuous-context explorations.
//! * **Built-in** — [`NoopAgent`] and [`FakeAgent`]. These require
//!   no external binary and run entirely in-process, exercising the
//!   real pipeline for verification testing.
//! * **Composite** — [`AgentRouter`] is itself an [`Agent`] that composes
//!   named sub-agents and dispatches to one per iteration according to a
//!   [`RoutingStrategy`]. It drives no CLI of its own; see [`router`] for
//!   the routing strategies.
//!
//! # No implicit defaults
//!
//! Every agent in this module is constructed directly from its fully
//! specified fields — there is no intermediate `*Settings` struct, and the
//! declaration → agent bind is a mechanical field move. None of them exposes
//! a `Default` impl or an implicit binary-name fallback.
//!
//! # Example
//!
//! ```no_run
//! use iter_core::agent::GenericAgent;
//! use iter_core::{Agent, AgentInvocation, Prompt};
//! use iter_core::signal::SignalId;
//! use std::path::Path;
//! use tokio_util::sync::CancellationToken;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let agent = GenericAgent::new(vec!["echo".into(), "hello".into()]);
//! let prompt = Prompt::from("x");
//! let ctx = AgentInvocation::new(
//!     Path::new("."),
//!     &prompt,
//!     CancellationToken::new(),
//!     SignalId::new(),
//! );
//! // `Ok` means the agent ran; a non-zero / failed run is `Err`.
//! let _run = agent.run(ctx).await?;
//! # Ok(()) }
//! ```

pub mod drivers;

pub(crate) mod cli_json;
pub mod command_path;
pub mod error;
mod hook_install;
pub mod inner;
pub mod kind;
pub mod mode;
pub(crate) mod process;
pub mod router;
pub mod run;
pub(crate) mod session;

#[cfg(test)]
mod testutil;

pub use drivers::antigravity::AntigravityAgent;
pub use drivers::claude::ClaudeAgent;
pub use drivers::cline::ClineAgent;
pub use drivers::codex::CodexAgent;
pub use drivers::copilot::CopilotAgent;
pub use drivers::cursor::CursorAgent;
pub use drivers::fake::FakeAgent;
pub use drivers::gemini::GeminiAgent;
pub use drivers::generic::GenericAgent;
pub use drivers::grok::GrokAgent;
pub use drivers::hermes::HermesAgent;
pub use drivers::noop::NoopAgent;
pub use drivers::opencode::OpenCodeAgent;
pub use error::AgentError;
pub use inner::{Agent, AgentInvocation, run_with_timeout};
pub use kind::AgentKind;
pub use mode::AgentMode;
pub use router::{AgentRouter, RoutingStrategy};
pub use run::AgentRun;
