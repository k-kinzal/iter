//! Coarse-grained classification of an agent run's outcome.
//!
//! `AgentOutcomeKind` lives alongside [`AgentReport`] and [`AgentError`]
//! because it classifies their values — it is agent-owned, not
//! process-owned. Both the runner's user-facing [`Event`] stream and its
//! system-facing [`RunnerLifecycle`] stream reference this type, but
//! neither stream defines it.
//!
//! [`AgentReport`]: super::AgentReport
//! [`AgentError`]: super::AgentError
//! [`Event`]: crate::runner::Event
//! [`RunnerLifecycle`]: crate::runner::RunnerLifecycle

use serde::{Deserialize, Serialize};

use super::error::AgentError;
use super::report::{AgentReport, ExitStatus};

/// Coarse-grained classification of an agent run's outcome.
///
/// The lifecycle stream does not carry agent stdout, stderr, or report
/// bodies — only this kind plus the optional exit code. This keeps the
/// lifecycle tracing channel bounded in size and free of user payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcomeKind {
    /// Agent process exited 0.
    Success,
    /// Agent process exited with a non-zero code.
    Failure,
    /// Agent process was terminated by a signal.
    TerminatedBySignal,
    /// Platform did not expose either an exit code or a terminating signal.
    UnknownExit,
    /// Agent run was stopped via cancellation. Covers both external
    /// cancellation (the runner's [`CancellationToken`] was fired by the
    /// caller) and internal cancellation (the runner fired an iter-scoped
    /// token because the iteration exceeded its configured timeout).
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    Cancelled,
    /// Agent run failed before producing a report (I/O error, missing
    /// command, hook setup failure, etc.).
    Errored,
    /// Agent hit the model's context-window or token limit.
    TokenLimit,
}

impl AgentOutcomeKind {
    /// Project a successful [`AgentReport`] into a coarse outcome kind.
    #[must_use]
    pub fn from_report(report: &AgentReport) -> Self {
        match report.exit_status {
            ExitStatus::Success => Self::Success,
            ExitStatus::Failure(_) => Self::Failure,
            ExitStatus::Signal(_) => Self::TerminatedBySignal,
            ExitStatus::Unknown => Self::UnknownExit,
        }
    }

    /// Project an [`AgentError`] into a coarse outcome kind.
    #[must_use]
    pub fn from_error(err: &AgentError) -> Self {
        match err {
            AgentError::Cancelled | AgentError::IterationTimeout(_) => Self::Cancelled,
            AgentError::UnknownExit => Self::UnknownExit,
            AgentError::TokenLimit(_) => Self::TokenLimit,
            AgentError::Io(_)
            | AgentError::EmptyCommand
            | AgentError::HookSetup(_) => Self::Errored,
        }
    }

    /// Project a `Result<&AgentReport, &AgentError>` into a coarse outcome
    /// kind. Convenience shortcut around [`Self::from_report`] /
    /// [`Self::from_error`].
    #[must_use]
    pub fn from_result(result: Result<&AgentReport, &AgentError>) -> Self {
        match result {
            Ok(rep) => Self::from_report(rep),
            Err(err) => Self::from_error(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_outcome_kind_maps_each_exit_status() {
        let mk = |status| AgentReport {
            exit_status: status,
            last_output: None,
            turn_count: None,
        };
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Success)),
            AgentOutcomeKind::Success
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Failure(2))),
            AgentOutcomeKind::Failure
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Signal(9))),
            AgentOutcomeKind::TerminatedBySignal
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Unknown)),
            AgentOutcomeKind::UnknownExit
        );
    }

    #[test]
    fn agent_outcome_kind_maps_each_error() {
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::Cancelled),
            AgentOutcomeKind::Cancelled
        );
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::UnknownExit),
            AgentOutcomeKind::UnknownExit
        );
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::TokenLimit("too large".into())),
            AgentOutcomeKind::TokenLimit
        );
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::EmptyCommand),
            AgentOutcomeKind::Errored
        );
        let io_err = std::io::Error::other("eio");
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::Io(io_err)),
            AgentOutcomeKind::Errored
        );
    }
}
