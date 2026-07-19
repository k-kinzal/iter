//! Durable semantic outcome stored alongside the operational status file.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use chrono::{DateTime, Utc};
use iter_core::{CompletionConditionInfo, RunnerExit, RunnerTerminationReason, SignalId};
use serde::Serialize;

use super::paths::{FILE_MODE, names};

/// Semantic result of an iter process.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RunOutcome {
    /// Exploration reached a normal completion boundary.
    Completed {
        /// Which normal boundary ended the runner.
        reason: CompletedReason,
        /// Redacted first-class condition, when condition-driven.
        #[serde(skip_serializing_if = "Option::is_none")]
        condition: Option<CompletionConditionInfo>,
        /// Attempted iteration count.
        iteration_count: u32,
        /// Last signal attempted.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_signal_id: Option<SignalId>,
        /// Time the condition was first latched.
        #[serde(skip_serializing_if = "Option::is_none")]
        requested_at: Option<DateTime<Utc>>,
        /// Time source disposition finished and this outcome became durable.
        completed_at: DateTime<Utc>,
    },
    /// Runner stopped without claiming exploration completion.
    Stopped {
        /// Why the runner stopped.
        reason: StoppedReason,
        /// Attempted iteration count.
        iteration_count: u32,
        /// Last signal attempted.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_signal_id: Option<SignalId>,
        /// Time the stopped outcome was recorded.
        stopped_at: DateTime<Utc>,
    },
    /// Runner or its operator-side finalization failed.
    Failed {
        /// Redacted/display error.
        message: String,
        /// Time the failure was recorded.
        failed_at: DateTime<Utc>,
    },
}

/// Normal completion boundary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletedReason {
    /// A declared condition was satisfied.
    Condition,
    /// CLI `--once` completed one attempted iteration.
    Once,
    /// The queue declared itself drained.
    QueueDrained,
}

/// Non-completing stop reason.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoppedReason {
    /// Cancellation token fired.
    Cancelled,
    /// A terminate-kind domain signal was consumed.
    TerminateSignal,
}

impl RunOutcome {
    /// Convert a successful typed runner exit into a durable semantic outcome.
    #[must_use]
    pub(crate) fn from_exit(exit: &RunnerExit, recorded_at: DateTime<Utc>) -> Self {
        match &exit.reason {
            RunnerTerminationReason::Completed { request } => Self::Completed {
                reason: CompletedReason::Condition,
                condition: Some(request.condition.clone()),
                iteration_count: exit.iteration_count,
                last_signal_id: exit.last_signal_id,
                requested_at: Some(request.requested_at),
                completed_at: recorded_at,
            },
            RunnerTerminationReason::Once => Self::Completed {
                reason: CompletedReason::Once,
                condition: None,
                iteration_count: exit.iteration_count,
                last_signal_id: exit.last_signal_id,
                requested_at: None,
                completed_at: recorded_at,
            },
            RunnerTerminationReason::QueueDrained => Self::Completed {
                reason: CompletedReason::QueueDrained,
                condition: None,
                iteration_count: exit.iteration_count,
                last_signal_id: exit.last_signal_id,
                requested_at: None,
                completed_at: recorded_at,
            },
            RunnerTerminationReason::Cancelled => Self::Stopped {
                reason: StoppedReason::Cancelled,
                iteration_count: exit.iteration_count,
                last_signal_id: exit.last_signal_id,
                stopped_at: recorded_at,
            },
            RunnerTerminationReason::TerminateSignalReceived => Self::Stopped {
                reason: StoppedReason::TerminateSignal,
                iteration_count: exit.iteration_count,
                last_signal_id: exit.last_signal_id,
                stopped_at: recorded_at,
            },
            RunnerTerminationReason::Error { message, .. } => Self::Failed {
                message: message.clone(),
                failed_at: recorded_at,
            },
        }
    }

    /// Construct an operator-side failure outcome.
    #[must_use]
    pub(crate) fn failed(message: String, recorded_at: DateTime<Utc>) -> Self {
        Self::Failed {
            message,
            failed_at: recorded_at,
        }
    }
}

/// Atomically publish `outcome.json` with mode 0600.
pub(crate) fn write_outcome(dir: &Path, outcome: &RunOutcome) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(outcome).map_err(io::Error::other)?;
    let temporary = dir.join(".outcome.json.tmp");
    let destination = dir.join(names::OUTCOME);
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temporary, &destination)?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::{
        CompletionConditionKind, CompletionRequest, IterationContext, RunnerTerminationReason,
    };

    fn condition_exit() -> RunnerExit {
        let now = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let request = CompletionRequest {
            condition: CompletionConditionInfo {
                name: "budget".into(),
                kind: CompletionConditionKind::Iterations,
                max: Some(3),
                duration_seconds: None,
                at: None,
            },
            iteration_count: 3,
            last_signal_id: None,
            requested_at: now,
            runner_started_at: now,
            elapsed_seconds: 2,
        };
        RunnerExit {
            reason: RunnerTerminationReason::Completed { request },
            iteration_count: 3,
            last_signal_id: None,
            iteration: IterationContext::for_count(3),
        }
    }

    #[test]
    fn writes_redacted_completed_outcome_atomically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorded_at = DateTime::parse_from_rfc3339("2026-07-19T12:01:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let outcome = RunOutcome::from_exit(&condition_exit(), recorded_at);
        write_outcome(dir.path(), &outcome).expect("write");

        let body =
            std::fs::read_to_string(dir.path().join(names::OUTCOME)).expect("outcome contents");
        let value: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(value["kind"], "completed");
        assert_eq!(value["reason"], "condition");
        assert_eq!(value["condition"]["name"], "budget");
        assert_eq!(value["condition"]["kind"], "iterations");
        assert_eq!(value["condition"]["max"], 3);
        assert!(!dir.path().join(".outcome.json.tmp").exists());
    }
}
