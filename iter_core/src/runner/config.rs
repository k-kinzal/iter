//! Configuration and summary types for the [`Runner`](super::Runner).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::ErrorSource;
use crate::signal::SignalId;

/// How the [`Runner`](super::Runner) acquires each Signal: wait for one to
/// cross the Queue boundary, or synthesise one itself.
///
/// `Wait` parks on the queue (the historical behaviour). `Synthesize` mints a
/// fresh Signal so the runner keeps iterating in the absence of upstream
/// Triggers — useful for ralph-loop-style continuous execution where the
/// runner has no queue at all.
///
/// `Synthesize` makes the Runner its own Signal source: a self-feeding Runner
/// is, in effect, its own Trigger. The tension is deliberate and named here
/// at the type rather than hidden — a Runner that synthesises its own work has
/// no outside widening it from within (see the crate's exploration model).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalAcquisition {
    /// Park on the queue until a signal arrives or cancellation fires.
    ///
    /// Requires a queue; combining `Wait` with no queue is rejected at
    /// builder time because there is nothing to wait on.
    #[default]
    Wait,
    /// Synthesise a signal whenever the queue is empty (or there is no
    /// queue at all) and continue iterating.
    ///
    /// `delay` is applied between iterations after the first one. It is
    /// not applied before the first iteration so a one-shot
    /// `behavior = loop` run still starts immediately.
    Synthesize {
        /// Sleep this long between successive synthesized iterations.
        ///
        /// `None` runs as fast as the runner allows (subject to a
        /// `tokio::task::yield_now` so single-threaded runtimes are not
        /// starved).
        delay: Option<Duration>,
    },
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
/// loop keeps iterating (`Synthesize`). See [`SignalAcquisition`] for the
/// full semantics. The combination `(queue=None, behavior=Wait)` is rejected
/// at builder time since there is nothing to wait on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerPolicy {
    /// Run exactly one signal then exit.
    pub once: bool,
    /// Continue processing further signals after a non-fatal error.
    pub continue_on_error: bool,
    /// What to do when no signal is available — wait on the queue or
    /// synthesise one.
    #[serde(default)]
    pub behavior: SignalAcquisition,
    /// Per-iteration timeout. When `Some(d)`, an iteration whose agent
    /// run lasts longer than `d` triggers an iter-scoped cancellation;
    /// the agent observes the cancel and terminates its process tree via
    /// [`ProcessGroup`], the agent run resolves to
    /// [`AgentError::Cancelled`], and the
    /// [`RunnerLifecycleEvent::AgentFinished`] event reports the `"cancelled"`
    /// result label. `None` (the default) leaves iterations unbounded.
    ///
    /// Absent in older NDJSON payloads: deserializes as `None`.
    ///
    /// [`ProcessGroup`]: crate::process_group::ProcessGroup
    /// [`RunnerLifecycleEvent::AgentFinished`]: crate::runner::RunnerLifecycleEvent::AgentFinished
    /// [`AgentError::Cancelled`]: crate::agent::AgentError::Cancelled
    #[serde(default)] // backward compat with payloads that predate this field
    pub iteration_timeout: Option<Duration>,
}

/// Reason the [`Runner`](super::Runner) loop terminated.
///
/// Renamed from the historical `TerminationReason` so it never collides
/// with the Process-side `ProcessTerminationReason` — the run record's
/// termination classification, owned by its operator (`iter_cli`'s
/// `process_lifecycle`).
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
    /// An error stopped the loop because `continue_on_error` was `false`.
    Error {
        /// Which of the iteration's operations produced the fatal error.
        ///
        /// Serialized as `"stage"` in the JSON wire format for backward
        /// compatibility with existing log consumers (R16 — the concept is
        /// the error source; only the legacy key name is pinned).
        #[serde(rename = "stage")]
        error_source: ErrorSource,
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
    /// Number of registered [`EventAction`](crate::EventAction) calls
    /// that returned `Err` during the run.
    ///
    /// The [`EventDispatcher`](crate::EventDispatcher) contract is best-effort,
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
    /// [`RunnerObserver`](crate::runner::RunnerObserver) calls that
    /// returned `Err` during the run.
    ///
    /// Parallel to [`Self::event_handler_error_count`] but for the
    /// **system** observer stream (the one that the runtime fans into
    /// the per-process NDJSON log sink via `tracing`). Per rev17 §F3 the
    /// runner emits observer-first, then user-emitter; both error counts
    /// are surfaced independently so a backed-up writer task is visible
    /// without drowning out user-handler failures.
    ///
    /// `#[serde(default)]` so old NDJSON payloads that pre-date this
    /// field still deserialize cleanly.
    #[serde(default)]
    pub observer_error_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_reason_error_serializes_stage_key() {
        let reason = RunnerTerminationReason::Error {
            error_source: ErrorSource::WorkspaceTeardown,
            message: "boom".into(),
        };
        let json = serde_json::to_value(&reason).expect("serialize");
        let inner = &json["Error"];
        assert_eq!(inner["stage"], "workspace_teardown");
        assert!(
            inner.get("error_source").is_none(),
            "field must serialize as 'stage', not 'error_source'"
        );
    }

    #[test]
    fn termination_reason_error_deserializes_from_legacy_stage_key() {
        let json = serde_json::json!({
            "Error": {
                "stage": "dequeue",
                "message": "oops"
            }
        });
        let reason: RunnerTerminationReason =
            serde_json::from_value(json).expect("deserialize legacy");
        assert_eq!(
            reason,
            RunnerTerminationReason::Error {
                error_source: ErrorSource::Dequeue,
                message: "oops".into(),
            }
        );
    }

    #[test]
    fn termination_reason_error_roundtrips() {
        let original = RunnerTerminationReason::Error {
            error_source: ErrorSource::AgentRun,
            message: "timeout".into(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let recovered: RunnerTerminationReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, recovered);
    }
}
