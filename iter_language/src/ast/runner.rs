//! `runner` declaration AST.

use super::event::EventHandlerDecl;
use super::prompt::PromptExpr;
use super::Spanned;

/// `runner { ... }` declaration — project-shaped runtime policy for the
/// iter loop.
///
/// In the new syntax, a runner binds named definitions by reference:
/// ```text
/// runner {
///     agent     = primary
///     workspace = dev
///     queue     = main
///     behavior  = loop
///     ...
/// }
/// ```
///
/// In the old (deprecated) flat syntax, these fields are absent and the
/// semantic analyzer synthesises them from the sole top-level definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerDecl {
    /// Optional runner name (for multi-runner files; currently unused at
    /// runtime but reserved in the AST for forward compatibility).
    pub name: Option<String>,
    /// Reference to a named agent definition.
    pub agent: String,
    /// Reference to a named workspace definition.
    pub workspace: String,
    /// Reference to a named queue definition (optional for loop-only runners).
    pub queue: Option<String>,
    /// If true, the runner continues after a stage failure; if
    /// false, one bad signal aborts the whole loop. Required — iter does
    /// not pick an error policy on the project's behalf.
    pub continue_on_error: bool,
    /// What to do when no signal is currently available on the queue (or
    /// when the runner has no queue at all). Required — iter does not
    /// pick a wait-vs-loop policy on the project's behalf.
    ///
    /// `wait` parks until a signal arrives; `loop { delay_secs = N }`
    /// synthesises an empty signal each iteration, optionally sleeping
    /// between iterations.
    pub behavior: RunnerBehavior,
    /// Optional per-iteration timeout in seconds. When set, an iteration
    /// that runs longer than this fires the iter-scoped cancel token,
    /// which kills the agent process tree and surfaces an
    /// `AgentError::IterationTimeout`. Use it as a runaway-iteration
    /// guard, not as an SLA — `continue_on_error` governs whether the
    /// runner moves on or breaks after a timeout.
    pub iteration_timeout_secs: Option<i64>,
    /// Prompt selection expression for this runner.
    pub prompt: PromptExpr,
    /// Event handlers scoped to this runner's lifecycle.
    pub events: Vec<Spanned<EventHandlerDecl>>,
}

/// Runner loop behaviour — what the runner does when no signal is
/// available to consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerBehavior {
    /// Block on `Queue::dequeue` until a signal arrives or the runner is
    /// cancelled. Requires a queue; `(no queue) + wait` is a semantic
    /// error.
    Wait,
    /// Synthesise an empty signal each iteration. When a queue is
    /// present, real signals on the queue are still preferred and the
    /// synthesis only fires on an empty queue. The optional `delay_secs`
    /// field controls how long to sleep between iterations (no sleep
    /// before the first iteration).
    Loop {
        /// Delay between iterations in seconds, or `None` for no delay.
        delay_secs: Option<i64>,
    },
}
