//! The agent cycle, the driver abstraction, and the driver implementations.
//!
//! Three levels, one direction of knowledge:
//!
//! * [`Agent`] — the iter noun. A struct (not a trait): it realises the
//!   agent **cycle** once — session resolution, prepare, spawn on the
//!   workspace, prompt delivery, faithful capture, cooperative termination,
//!   interpretation, guaranteed cleanup. Runs on an
//!   [`ActiveWorkspace`](crate::workspace::ActiveWorkspace) via
//!   [`Agent::run_on`].
//! * [`AgentDriver`] — the bidirectional translator for one agent CLI:
//!   [`command`](AgentDriver::command) encodes iter's intent into the CLI's
//!   dialect, [`interpret`](AgentDriver::interpret) decodes the CLI's own
//!   verdict back into iter's vocabulary. Drivers never touch a process.
//! * The concrete drivers under [`drivers`] — one per CLI (thirteen today),
//!   named after the product they translate for ([`ClaudeCodeDriver`],
//!   [`CodexDriver`], …).
//!
//! Driver selection is uniform: an `Agent` always holds one [`Router`]
//! ([`SingleAgentRouter`] when there is nothing to route,
//! [`FallbackRouter`] / [`RotateRouter`] for composition). See [`router`].
//!
//! Driver groups:
//!
//! * **Hook-capable** — [`ClaudeCodeDriver`], [`CodexDriver`],
//!   [`GeminiDriver`], and [`CopilotDriver`] each run in either
//!   [`AgentMode::Headless`] (piped one-shot invocation whose
//!   machine-readable output `interpret` parses) or
//!   [`AgentMode::Interactive`] (live TUI session driven by a project-local
//!   Stop-style hook installed in `prepare` and restored in `cleanup`).
//! * **Print-only** — [`CursorDriver`], [`ClineDriver`],
//!   [`OpenCodeDriver`], [`GrokDriver`], and [`GenericDriver`]: piped
//!   one-shot invocations with no hook lifecycle.
//! * **Headless/interactive without hooks** — [`HermesDriver`] and
//!   [`AntigravityDriver`].
//! * **Shell-synthesised** — [`NoopDriver`] and [`FakeDriver`]: translate
//!   to trivial `sh` invocations so pipelines can be exercised without a
//!   real AI CLI, through the exact same spawn path.
//!
//! # No implicit defaults
//!
//! Every driver in this module is constructed directly from its fully
//! specified fields — there is no intermediate `*Settings` struct, and the
//! declaration → driver bind is a mechanical field move. None of them exposes
//! a `Default` impl or an implicit binary-name fallback.
//!
//! # Example
//!
//! ```no_run
//! use iter_core::agent::{Agent, GenericDriver, SingleAgentRouter};
//! use iter_core::workspace::{LocalWorkspace, Workspace};
//! use iter_core::Prompt;
//! use tokio_util::sync::CancellationToken;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let driver = GenericDriver::new(vec!["echo".into(), "hello".into()]);
//! let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(driver))));
//!
//! let mut workspace = LocalWorkspace::new(".");
//! let active = Workspace::setup(&mut workspace, CancellationToken::new()).await?;
//! let prompt = Prompt::from("x");
//! // `Ok` means the agent ran; a non-zero / failed run is `Err`.
//! let _run = agent.run_on(&*active, &prompt, CancellationToken::new()).await?;
//! let _persistent = active.teardown(CancellationToken::new()).await?;
//! # Ok(()) }
//! ```

pub mod drivers;

// Defining module named for the concept it defines — the path echo is deliberate.
pub mod agent;
pub(crate) mod cli_json;
pub mod driver;
pub mod error;
mod hook_install;
pub mod kind;
pub mod mode;
pub(crate) mod process;
pub mod router;
pub mod run;
pub(crate) mod session;

#[cfg(test)]
mod testutil;

pub use agent::Agent;
pub use driver::{AgentCommand, AgentDriver};
pub use drivers::antigravity::AntigravityDriver;
pub use drivers::claude_code::ClaudeCodeDriver;
pub use drivers::cline::ClineDriver;
pub use drivers::codex::CodexDriver;
pub use drivers::copilot::CopilotDriver;
pub use drivers::cursor::CursorDriver;
pub use drivers::fake::FakeDriver;
pub use drivers::gemini::GeminiDriver;
pub use drivers::generic::GenericDriver;
pub use drivers::grok::GrokDriver;
pub use drivers::hermes::HermesDriver;
pub use drivers::noop::NoopDriver;
pub use drivers::opencode::OpenCodeDriver;
pub use error::{AgentError, FallbackClass};
pub use kind::AgentKind;
pub use mode::AgentMode;
pub use router::{
    FallbackRouter, FallbackTriggers, RotateRouter, Route, Router, SingleAgentRouter,
};
pub use run::{AgentOutput, AgentRun};
