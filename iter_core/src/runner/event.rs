//! [`Event`] stream produced by the [`Runner`](crate::runner::Runner).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::AgentReport;
use crate::prompt::Prompt;
use crate::runner::config::RunnerTerminationReason;
use crate::signal::{Signal, SignalId};

/// Event emitted by the [`Runner`](crate::runner::Runner) between every step
/// of the per-signal loop.
///
/// Per-signal lifecycle events carry the full [`Signal`] (not just its id)
/// so that [`EventHandler`](crate::runner::EventHandler) implementations —
/// such as template-rendering shell handlers — have direct access to the
/// signal's metadata without needing an external lookup table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The runner is about to enter its per-signal loop. Fired exactly once,
    /// before any signal is dequeued and before any other lifecycle event.
    ///
    /// Has no signal context — handlers wired to `on runner_starting` cannot
    /// reference `{{signal.*}}` template variables.
    RunnerStarting {},
    /// A signal was successfully dequeued.
    SignalReceived {
        /// The dequeued signal.
        signal: Signal,
    },
    /// The runner is about to call [`Workspace::setup`](crate::workspace::Workspace::setup).
    WorkspaceSetupStarting {
        /// Signal currently being handled.
        signal: Signal,
    },
    /// The workspace finished setup.
    WorkspaceSetupFinished {
        /// Signal currently being handled.
        signal: Signal,
        /// Filesystem path of the prepared workspace.
        path: PathBuf,
    },
    /// The runner is about to invoke the agent.
    AgentStarting {
        /// Signal currently being handled.
        signal: Signal,
        /// Workspace path supplied to the agent.
        path: PathBuf,
        /// Rendered prompt supplied to the agent.
        prompt: Prompt,
    },
    /// The agent run completed (successfully or not).
    AgentFinished {
        /// Signal currently being handled.
        signal: Signal,
        /// Workspace path supplied to the agent.
        path: PathBuf,
        /// Result of the agent run, with the error stringified.
        report: Result<AgentReport, String>,
    },
    /// The runner is about to tear down the workspace.
    WorkspaceTeardownStarting {
        /// Signal currently being handled.
        signal: Signal,
        /// Workspace path that will be torn down.
        path: PathBuf,
    },
    /// The workspace teardown finished.
    ///
    /// Carries the `path` of the (now torn-down) workspace because this is
    /// the canonical place for commit-on-teardown shell handlers to run
    /// `git` commands — and they need a cwd, not just a signal id.
    WorkspaceTeardownFinished {
        /// Signal currently being handled.
        signal: Signal,
        /// Filesystem path of the workspace that was torn down.
        path: PathBuf,
    },
    /// A dequeue operation failed.
    DequeueFailed {
        /// Stringified error message.
        error: String,
    },
    /// Prompt rendering failed for a signal.
    RenderPromptFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Workspace setup failed for a signal.
    WorkspaceSetupFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Agent run failed for a signal.
    AgentRunFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Workspace teardown failed for a signal.
    WorkspaceTeardownFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// The runner has finished its per-signal loop and is about to return
    /// from [`Runner::run`](crate::runner::Runner::run). Fired exactly once
    /// regardless of termination reason — including `RunnerExitError` exit
    /// paths.
    RunnerFinished {
        /// Why the runner loop terminated.
        reason: RunnerTerminationReason,
        /// Number of signals processed (whether successfully or not).
        iteration_count: u32,
    },
}

/// Routing key for event dispatch.
///
/// Each variant names a logical event the runner emits. The emitter
/// uses this to invoke only the handlers registered for a given name
/// rather than broadcasting to all handlers.
///
/// The mapping from [`Event`] to `EventName` is defined by
/// [`Event::name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventName {
    /// `runner_starting`
    RunnerStarting,
    /// `signal_received`
    SignalReceived,
    /// `workspace_setup_starting`
    WorkspaceSetupStarting,
    /// `workspace_setup_finished`
    WorkspaceSetupFinished,
    /// `agent_starting`
    AgentStarting,
    /// `agent_finished`
    AgentFinished,
    /// `workspace_teardown_starting`
    WorkspaceTeardownStarting,
    /// `workspace_teardown_finished`
    WorkspaceTeardownFinished,
    /// `runner_error` — covers all error variants.
    RunnerError,
    /// `runner_finished`
    RunnerFinished,
}

impl EventName {
    /// All event name variants.
    pub const ALL: &'static [EventName] = &[
        EventName::RunnerStarting,
        EventName::SignalReceived,
        EventName::WorkspaceSetupStarting,
        EventName::WorkspaceSetupFinished,
        EventName::AgentStarting,
        EventName::AgentFinished,
        EventName::WorkspaceTeardownStarting,
        EventName::WorkspaceTeardownFinished,
        EventName::RunnerError,
        EventName::RunnerFinished,
    ];
}

impl Event {
    /// The routing key for this event.
    ///
    /// All error variants (`DequeueFailed`, `RenderPromptFailed`,
    /// `WorkspaceSetupFailed`, `AgentRunFailed`,
    /// `WorkspaceTeardownFailed`) map to [`EventName::RunnerError`].
    #[must_use]
    pub fn name(&self) -> EventName {
        match self {
            Self::RunnerStarting {} => EventName::RunnerStarting,
            Self::SignalReceived { .. } => EventName::SignalReceived,
            Self::WorkspaceSetupStarting { .. } => EventName::WorkspaceSetupStarting,
            Self::WorkspaceSetupFinished { .. } => EventName::WorkspaceSetupFinished,
            Self::AgentStarting { .. } => EventName::AgentStarting,
            Self::AgentFinished { .. } => EventName::AgentFinished,
            Self::WorkspaceTeardownStarting { .. } => EventName::WorkspaceTeardownStarting,
            Self::WorkspaceTeardownFinished { .. } => EventName::WorkspaceTeardownFinished,
            Self::DequeueFailed { .. }
            | Self::RenderPromptFailed { .. }
            | Self::WorkspaceSetupFailed { .. }
            | Self::AgentRunFailed { .. }
            | Self::WorkspaceTeardownFailed { .. } => EventName::RunnerError,
            Self::RunnerFinished { .. } => EventName::RunnerFinished,
        }
    }
}

/// Label identifying which runner step produced an error.
///
/// Used in [`RunnerExitError`](crate::runner::RunnerExitError),
/// [`RunnerTerminationReason`], and
/// [`RunnerLifecycle`](crate::runner::RunnerLifecycle) for
/// display and serialization. Not a behavioral dispatch mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorStage {
    /// Pulling a signal off the queue.
    Dequeue,
    /// Rendering a prompt template.
    RenderPrompt,
    /// Setting up the workspace.
    WorkspaceSetup,
    /// Running the agent.
    AgentRun,
    /// Tearing down the workspace.
    WorkspaceTeardown,
}

impl std::fmt::Display for ErrorStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Dequeue => "dequeue",
            Self::RenderPrompt => "render_prompt",
            Self::WorkspaceSetup => "workspace_setup",
            Self::AgentRun => "agent_run",
            Self::WorkspaceTeardown => "workspace_teardown",
        };
        f.write_str(label)
    }
}

impl Event {
    /// Return the signal id associated with this event, if any.
    ///
    /// Useful for log formatting and cross-event grouping.
    #[must_use]
    pub fn signal_id(&self) -> Option<SignalId> {
        match self {
            Self::SignalReceived { signal }
            | Self::WorkspaceSetupStarting { signal }
            | Self::WorkspaceSetupFinished { signal, .. }
            | Self::AgentStarting { signal, .. }
            | Self::AgentFinished { signal, .. }
            | Self::WorkspaceTeardownStarting { signal, .. }
            | Self::WorkspaceTeardownFinished { signal, .. } => Some(signal.id()),
            Self::RenderPromptFailed { signal_id, .. }
            | Self::WorkspaceSetupFailed { signal_id, .. }
            | Self::AgentRunFailed { signal_id, .. }
            | Self::WorkspaceTeardownFailed { signal_id, .. } => Some(*signal_id),
            Self::DequeueFailed { .. } | Self::RunnerStarting {} | Self::RunnerFinished { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_name_all_covers_every_variant() {
        let all_set: std::collections::HashSet<EventName> =
            EventName::ALL.iter().copied().collect();
        // Exhaustive match — adding a variant without listing it here
        // causes a compile error.
        for &name in EventName::ALL {
            match name {
                EventName::RunnerStarting
                | EventName::SignalReceived
                | EventName::WorkspaceSetupStarting
                | EventName::WorkspaceSetupFinished
                | EventName::AgentStarting
                | EventName::AgentFinished
                | EventName::WorkspaceTeardownStarting
                | EventName::WorkspaceTeardownFinished
                | EventName::RunnerError
                | EventName::RunnerFinished => {}
            }
        }
        assert_eq!(
            all_set.len(),
            EventName::ALL.len(),
            "ALL contains duplicates",
        );
        assert_eq!(
            EventName::ALL.len(),
            10,
            "EventName variant count changed — update ALL",
        );
    }
}
