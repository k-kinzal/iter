//! `runner` declaration AST.

use super::Spanned;
use super::event::EventHandlerDef;
use super::prompt::PromptExpr;

/// `runner { ... }` declaration — project-shaped runtime policy for the
/// iter loop.
///
/// A runner binds named definitions by reference and carries the prompt
/// selection plus lifecycle event handlers:
/// ```text
/// runner {
///     agent     = primary
///     workspace = dev
///     queue     = main
///     behavior  = loop
///     ...
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerDef {
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
    pub behavior: SignalAcquisition,
    /// Optional per-iteration timeout in seconds. When set, an iteration
    /// that runs longer than this fires the iter-scoped cancel token,
    /// which kills the agent process tree and surfaces an
    /// `AgentError::IterationTimeout`. Use it as a runaway-iteration
    /// guard, not as an SLA — `continue_on_error` governs whether the
    /// runner moves on or breaks after a timeout.
    pub iteration_timeout_secs: Option<i64>,
    /// Optional first-class completion policy. Conditions are evaluated as
    /// an ordered OR-set: the first satisfied condition requests completion.
    pub completion: Option<CompletionDef>,
    /// Prompt selection expression for this runner.
    pub prompt: PromptExpr,
    /// Event handlers scoped to this runner's lifecycle.
    pub events: Vec<Spanned<EventHandlerDef>>,
}

/// Ordered completion conditions attached to a runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionDef {
    /// Conditions in declaration order.
    pub conditions: Vec<Spanned<CompletionConditionDef>>,
}

/// One externally authored runner completion condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionConditionDef {
    /// Complete after at least `max` attempted iterations.
    Iterations {
        /// Stable user-facing condition name.
        name: String,
        /// Maximum attempted iteration count.
        max: u32,
    },
    /// Run a shell predicate after iteration boundaries.
    Shell {
        /// Stable user-facing condition name.
        name: String,
        /// Shell command. Exit 0 means satisfied; 1 means pending.
        run: String,
        /// Predicate timeout in seconds.
        timeout_secs: u64,
        /// Policy for exit codes other than 0/1 and execution errors.
        on_error: CompletionConditionErrorPolicy,
    },
    /// Complete after a monotonic duration from runner start.
    Elapsed {
        /// Stable user-facing condition name.
        name: String,
        /// Positive elapsed duration in seconds.
        duration_secs: u64,
    },
    /// Complete at an absolute RFC 3339 instant.
    Deadline {
        /// Stable user-facing condition name.
        name: String,
        /// Validated RFC 3339 timestamp with an explicit UTC offset.
        at: String,
    },
}

/// Error policy for shell completion predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionConditionErrorPolicy {
    /// Abort the runner when the predicate cannot produce a decision.
    Abort,
    /// Log the predicate error and keep the condition pending.
    Continue,
}

/// Runner loop behaviour — what the runner does when no signal is
/// available to consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalAcquisition {
    /// Block on `Queue::dequeue` until a signal arrives or the runner is
    /// cancelled. Requires a queue; `(no queue) + wait` is a semantic
    /// error.
    Wait,
    /// Synthesise an empty signal each iteration. When a queue is
    /// present, real signals on the queue are still preferred and the
    /// synthesis only fires on an empty queue. The optional `delay_secs`
    /// field controls how long to sleep between iterations (no sleep
    /// before the first iteration).
    ///
    /// Spelled `loop { … }` in the grammar (the surface keyword is kept; the
    /// AST variant names the concept).
    Synthesize {
        /// Delay between iterations in seconds, or `None` for no delay.
        delay_secs: Option<i64>,
    },
}
