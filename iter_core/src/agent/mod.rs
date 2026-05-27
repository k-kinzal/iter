//! Agent trait, shared primitives, and driver implementations.
//!
//! The [`Agent`] trait and associated types live at this level. Concrete
//! implementations live under [`drivers`]. Shared OS-level primitives
//! (subprocess management, hook lifecycle, transcript parsing) are
//! `pub(crate)` internal modules used by the drivers.
//!
//! This module provides nine concrete implementations of the
//! [`Agent`] trait. They fall into two broad groups based on how
//! the underlying CLI is driven:
//!
//! * **Hook-capable** — [`ClaudeAgent`], [`CodexAgent`], [`GeminiAgent`],
//!   [`AntigravityAgent`], and [`CopilotAgent`] each run in either
//!   [`AgentMode::Print`]
//!   (non-interactive one-shot invocation that captures stdout into
//!   [`AgentReport::last_output`]) or
//!   [`AgentMode::Interactive`] (live TUI session driven by a
//!   project-local Stop-style hook installed under the agent's
//!   own config directory).
//! * **Print-only** — [`CursorAgent`], [`ClineAgent`], [`OpenCodeAgent`],
//!   and [`GenericAgent`]. These tools run to completion on every
//!   invocation with no hook plumbing.
//!
//! # No implicit defaults
//!
//! Every agent in this module is constructed from a fully-populated
//! `*Settings` struct. None of them exposes a `Default` impl or an
//! implicit binary-name fallback.
//!
//! # Example
//!
//! ```no_run
//! use iter_core::agent::GenericAgent;
//! use iter_core::{Agent, AgentRunContext, Prompt};
//! use iter_core::signal::SignalId;
//! use std::path::Path;
//! use tokio_util::sync::CancellationToken;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let agent = GenericAgent::new(vec!["echo".into(), "hello".into()]);
//! let prompt = Prompt::from("x");
//! let ctx = AgentRunContext::new(
//!     Path::new("."),
//!     &prompt,
//!     CancellationToken::new(),
//!     SignalId::new(),
//! );
//! let report = agent.run(ctx).await?;
//! assert!(report.exit_status.is_success());
//! # Ok(()) }
//! ```

pub mod drivers;

pub mod command_path;
pub mod error;
mod hook_lifecycle;
pub mod inner;
pub mod mode;
pub mod outcome;
pub(crate) mod process;
pub mod report;
mod transcript;

#[cfg(test)]
mod testutil;

pub use drivers::antigravity::{AntigravityAgent, AntigravitySettings};
pub use drivers::claude::{ClaudeAgent, ClaudeSettings};
pub use drivers::cline::{ClineAgent, ClineSettings};
pub use drivers::codex::{CodexAgent, CodexSettings};
pub use drivers::copilot::{CopilotAgent, CopilotSettings};
pub use drivers::cursor::{CursorAgent, CursorSettings};
pub use drivers::gemini::{GeminiAgent, GeminiSettings};
pub use drivers::generic::GenericAgent;
pub use drivers::opencode::{OpenCodeAgent, OpenCodeSettings};
pub use error::AgentError;
pub use inner::{Agent, AgentRunContext, run_with_timeout};
pub use mode::AgentMode;
pub use outcome::AgentOutcomeKind;
pub use report::{AgentReport, ExitStatus};
