//! Configuration and summary types for the [`Runner`](super::Runner).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::event::ErrorStage;
use crate::signal::SignalId;

/// What the [`Runner`](super::Runner) should do when no signal is available.
///
/// `Wait` parks on the queue (the historical behaviour). `Loop` synthesises
/// a fresh signal so the runner keeps iterating in the absence of upstream
/// triggers — useful for ralph-loop-style continuous execution where the
/// runner has no queue at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerBehavior {
    /// Park on the queue until a signal arrives or cancellation fires.
    ///
    /// Requires a queue; combining `Wait` with no queue is rejected at
    /// builder time because there is nothing to wait on.
    Wait,
    /// Synthesise a signal whenever the queue is empty (or there is no
    /// queue at all) and continue iterating.
    ///
    /// `delay` is applied between iterations after the first one. It is
    /// not applied before the first iteration so a one-shot
    /// `behavior = loop` run still starts immediately.
    Loop {
        /// Sleep this long between successive synthesized iterations.
        ///
        /// `None` runs as fast as the runner allows (subject to a
        /// `tokio::task::yield_now` so single-threaded runtimes are not
        /// starved).
        delay: Option<Duration>,
    },
}

impl Default for RunnerBehavior {
    fn default() -> Self {
        Self::Wait
    }
}

/// Behaviour switches for the [`Runner`](super::Runner) loop.
///
/// The Runner's termination model is deliberately Signal-centric: the loop
/// stops when the queue drains, when the cancel token fires, or when the
/// operator passes `--once` to process exactly one signal. There are no
/// output-sniffing or shell-exit termination conditions — if a workflow
/// should stop on an external condition, author a Trigger that stops
/// producing signals (or invert the pattern with a shutdown-signal Trigger).
///
/// `behavior` controls what the runner does when no signal is available:
/// either park on the queue (`Wait`) or synthesise a fresh signal so the
/// loop keeps iterating (`Loop`). See [`RunnerBehavior`] for the full
/// semantics. The combination `(queue=None, behavior=Wait)` is rejected at
/// builder time since there is nothing to wait on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// Run exactly one signal then exit.
    pub once: bool,
    /// Continue processing further signals after a non-fatal runner stage error.
    pub continue_on_error: bool,
    /// What to do when no signal is available — wait on the queue or
    /// synthesise one.
    #[serde(default)]
    pub behavior: RunnerBehavior,
    /// Per-iteration timeout. When `Some(d)`, an iteration whose agent
    /// run lasts longer than `d` triggers an iter-scoped cancellation;
    /// the agent observes the cancel and terminates its process tree via
    /// [`ProcessGroup`], and the [`RunnerLifecycle::AgentFinished`] event
    /// reports [`AgentOutcomeKind::Cancelled`]. `None` (the default)
    /// leaves iterations unbounded.
    ///
    /// Absent in older NDJSON payloads: deserializes as `None`.
    ///
    /// [`ProcessGroup`]: crate::process::ProcessGroup
    /// [`RunnerLifecycle::AgentFinished`]: crate::process::RunnerLifecycle::AgentFinished
    /// [`AgentOutcomeKind::Cancelled`]: crate::process::lifecycle::AgentOutcomeKind::Cancelled
    #[serde(default)] // backward compat with payloads that predate this field
    pub iteration_timeout: Option<Duration>,
}

/// Reason the [`Runner`](super::Runner) loop terminated.
///
/// Renamed from the historical `TerminationReason` so it never collides
/// with the Process-side
/// [`ProcessTerminationReason`](crate::process::ProcessTerminationReason).
/// The two live at different layers and need different vocabularies:
/// `RunnerTerminationReason` describes why the *Runner* loop stopped,
/// `ProcessTerminationReason` describes why the *Process* (the OS-level
/// host of the runner) stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerTerminationReason {
    /// The supplied cancellation token fired.
    Cancelled,
    /// `once` was set and one signal was processed.
    Once,
    /// The queue returned `None` from `dequeue`, signalling no more work.
    QueueDrained,
    /// A [`SignalKind::Terminate`](crate::signal::SignalKind::Terminate)
    /// signal was dequeued. The runner exits gracefully without invoking
    /// the agent for that signal.
    TerminateSignalReceived,
    /// A runner stage error stopped the loop because `continue_on_error`
    /// was `false`.
    Error {
        /// Which runner step produced the fatal error.
        stage: ErrorStage,
        /// Stringified error message.
        message: String,
    },
}

/// Summary returned by [`Runner::run`](super::Runner::run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerSummary {
    /// Number of signals processed (whether successfully or not).
    pub iteration_count: u32,
    /// Identifier of the last signal attempted, regardless of whether
    /// processing succeeded. `None` if no signal was ever pulled from
    /// the queue.
    pub last_signal_id: Option<SignalId>,
    /// Why the loop terminated.
    pub termination_reason: RunnerTerminationReason,
    /// Number of registered [`EventHandler`](crate::EventHandler) calls
    /// that returned `Err` during the run.
    ///
    /// The [`EventEmitter`](crate::EventEmitter) contract is best-effort,
    /// so a failing handler does not halt the runner; this counter
    /// surfaces the silent failures so a run with a broken
    /// `on workspace_teardown_finished { shell "..." }` handler can no longer
    /// finish with a clean summary. Each individual error is also logged
    /// via `tracing` at `warn` level with the handler index.
    ///
    /// `#[serde(default)]` so old NDJSON payloads that pre-date this
    /// field still deserialize cleanly into the current struct.
    #[serde(default)]
    pub event_handler_error_count: u32,
    /// Number of system-contract
    /// [`RunnerObserver`](crate::process::RunnerObserver) calls that
    /// returned `Err` during the run.
    ///
    /// Parallel to [`Self::event_handler_error_count`] but for the
    /// **system** observer stream (the one that the runtime fans into
    /// `~/.iter/proc/<id>/log.ndjson` via `tracing`). Per rev17 §F3 the
    /// runner emits observer-first, then user-emitter; both error counts
    /// are surfaced independently so a backed-up writer task is visible
    /// without drowning out user-handler failures.
    ///
    /// `#[serde(default)]` so old NDJSON payloads that pre-date this
    /// field still deserialize cleanly.
    #[serde(default)]
    pub observer_error_count: u32,
}
