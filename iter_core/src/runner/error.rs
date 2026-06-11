//! [`ErrorSource`] and [`RunnerExitError`] — the failure diagnostics of one
//! iteration.

use serde::{Deserialize, Serialize};

/// Which operation within an iteration produced an error.
///
/// An iteration is one act — receive a Signal, render the prompt, set up the
/// Workspace, run the Agent, tear the Workspace down — and `ErrorSource`
/// answers *which of those operations failed*. It is a failure diagnostic,
/// **not** a model of the run as a sequence of phases: iter is not a workflow
/// system, nothing travels through phases, and a successful iteration has no
/// parts to enumerate. The value exists only on the failure path.
///
/// # Wire format
///
/// Serialized as the JSON key `"stage"` on the `runner_error` and
/// `runner_finished` records for backward compatibility with existing log
/// consumers — the *concept* is the error source; only the legacy key name is
/// pinned. Changing the key is a wire migration, not a rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    /// Pulling a Signal off the Queue.
    Dequeue,
    /// Rendering the prompt template.
    RenderPrompt,
    /// Setting up the Workspace.
    WorkspaceSetup,
    /// Running the Agent.
    AgentRun,
    /// Tearing down the Workspace.
    WorkspaceTeardown,
}

impl ErrorSource {
    /// The wire/log form (`snake_case`), e.g. `"workspace_setup"`.
    ///
    /// Matches the serialized `"stage"` value and the `iter.error.source`
    /// tracing attribute. Distinct from the [`Display`](std::fmt::Display)
    /// form, which is the spaced, human-readable label used inside error
    /// messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dequeue => "dequeue",
            Self::RenderPrompt => "render_prompt",
            Self::WorkspaceSetup => "workspace_setup",
            Self::AgentRun => "agent_run",
            Self::WorkspaceTeardown => "workspace_teardown",
        }
    }

    /// Human-readable form used in error messages, e.g. `"workspace setup"`.
    fn label(self) -> &'static str {
        match self {
            Self::Dequeue => "dequeue",
            Self::RenderPrompt => "render prompt",
            Self::WorkspaceSetup => "workspace setup",
            Self::AgentRun => "agent run",
            Self::WorkspaceTeardown => "workspace teardown",
        }
    }
}

impl std::fmt::Display for ErrorSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Error returned by [`super::Runner::run`] when a fatal error stops the loop.
///
/// One shape for every failing operation: the [`ErrorSource`] says *which*
/// operation failed, the rest carries the failure detail. (Earlier revisions
/// re-encoded the operation as a parallel set of variants; the source is a
/// single diagnostic field now, not a taxonomy.)
#[derive(Debug, thiserror::Error)]
#[error("{error_source} failed: {message}")]
pub struct RunnerExitError {
    /// Which of the iteration's operations produced the fatal error.
    pub error_source: ErrorSource,
    /// Stringified source error.
    pub message: String,
    /// Boxed original source error.
    #[source]
    pub source: Box<dyn std::error::Error + Send + Sync + 'static>,
    /// Running tally of event-action errors across the run.
    pub event_handler_error_count: u32,
    /// Running tally of observer errors across the run.
    pub observer_error_count: u32,
}

impl RunnerExitError {
    /// Which of the iteration's operations produced this error.
    #[must_use]
    pub fn error_source(&self) -> ErrorSource {
        self.error_source
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn with_counters(
        mut self,
        event_handler_error_count: u32,
        observer_error_count: u32,
    ) -> Self {
        self.event_handler_error_count = event_handler_error_count;
        self.observer_error_count = observer_error_count;
        self
    }
}
