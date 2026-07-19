//! First-class runner completion conditions and completion records.

use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::Instant;

use super::iteration::IterationContext;
use super::policy::RunnerTerminationReason;
use crate::process_group::{self, ProcessGroup};
use crate::signal::SignalId;

/// Error handling policy for a shell completion predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionConditionErrorPolicy {
    /// A predicate execution error aborts the runner.
    Abort,
    /// A predicate execution error is logged and the condition remains pending.
    Continue,
}

/// A condition that can request semantic completion of a runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionCondition {
    /// Complete after `max` attempted iterations.
    Iterations {
        /// Stable condition name.
        name: String,
        /// Maximum attempted iteration count.
        max: u32,
    },
    /// Evaluate an external shell predicate at iteration boundaries.
    Shell {
        /// Stable condition name.
        name: String,
        /// Command passed to `sh -c`. It is deliberately absent from
        /// [`CompletionConditionInfo`] and durable outcome records.
        command: String,
        /// Maximum predicate runtime.
        timeout: Duration,
        /// Policy for execution errors and exits other than 0/1.
        on_error: CompletionConditionErrorPolicy,
    },
    /// Complete after a monotonic duration from runner start.
    Elapsed {
        /// Stable condition name.
        name: String,
        /// Positive duration from runner start.
        duration: Duration,
    },
    /// Complete at an absolute instant.
    Deadline {
        /// Stable condition name.
        name: String,
        /// Absolute UTC instant.
        at: DateTime<Utc>,
    },
}

/// Stable completion condition kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionConditionKind {
    /// Iteration budget.
    Iterations,
    /// External shell predicate.
    Shell,
    /// Monotonic elapsed-time budget.
    Elapsed,
    /// Absolute wall-clock deadline.
    Deadline,
}

impl CompletionConditionKind {
    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iterations => "iterations",
            Self::Shell => "shell",
            Self::Elapsed => "elapsed",
            Self::Deadline => "deadline",
        }
    }
}

/// Redacted, serializable description of the condition that completed a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionConditionInfo {
    /// Stable user-facing condition name.
    pub name: String,
    /// Condition kind.
    pub kind: CompletionConditionKind,
    /// Iteration budget, for `iterations`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// Duration in seconds, for `elapsed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// RFC 3339 UTC instant, for `deadline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

/// A latched request to complete the runner at its next safe boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Redacted condition that requested completion.
    pub condition: CompletionConditionInfo,
    /// Attempted iteration count when the request was latched.
    pub iteration_count: u32,
    /// Last signal attempted, if any.
    pub last_signal_id: Option<SignalId>,
    /// Wall-clock time at which the request was latched.
    pub requested_at: DateTime<Utc>,
    /// Wall-clock runner start.
    pub runner_started_at: DateTime<Utc>,
    /// Monotonic elapsed whole seconds at request time.
    pub elapsed_seconds: u64,
}

/// Completion data carried by runner completion lifecycle hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionEvent {
    /// The original completion request.
    #[serde(flatten)]
    pub request: CompletionRequest,
    /// Set only for `runner_completed`, after finalization is durable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Typed result of a runner loop.
#[derive(Debug, Clone)]
pub struct RunnerExit {
    /// Why the loop stopped.
    pub reason: RunnerTerminationReason,
    /// Number of attempted iterations.
    pub iteration_count: u32,
    /// Last signal attempted, if any.
    pub last_signal_id: Option<SignalId>,
    /// Final template snapshot for deferred lifecycle handlers.
    pub iteration: IterationContext,
}

impl RunnerExit {
    /// Completion request when the exit was condition-driven.
    #[must_use]
    pub fn completion_request(&self) -> Option<&CompletionRequest> {
        match &self.reason {
            RunnerTerminationReason::Completed { request } => Some(request),
            RunnerTerminationReason::Cancelled
            | RunnerTerminationReason::Once
            | RunnerTerminationReason::QueueDrained
            | RunnerTerminationReason::TerminateSignalReceived
            | RunnerTerminationReason::Error { .. } => None,
        }
    }
}

impl CompletionEvent {
    /// Create the pre-finalization `runner_completing` payload.
    #[must_use]
    pub fn completing(request: CompletionRequest) -> Self {
        Self {
            request,
            completed_at: None,
        }
    }

    /// Create the durable `runner_completed` payload.
    #[must_use]
    pub fn completed(request: CompletionRequest, completed_at: DateTime<Utc>) -> Self {
        Self {
            request,
            completed_at: Some(completed_at),
        }
    }
}

impl CompletionCondition {
    pub(super) fn info(&self) -> CompletionConditionInfo {
        match self {
            Self::Iterations { name, max } => CompletionConditionInfo {
                name: name.clone(),
                kind: CompletionConditionKind::Iterations,
                max: Some(*max),
                duration_seconds: None,
                at: None,
            },
            Self::Shell { name, .. } => CompletionConditionInfo {
                name: name.clone(),
                kind: CompletionConditionKind::Shell,
                max: None,
                duration_seconds: None,
                at: None,
            },
            Self::Elapsed { name, duration } => CompletionConditionInfo {
                name: name.clone(),
                kind: CompletionConditionKind::Elapsed,
                max: None,
                duration_seconds: Some(duration.as_secs()),
                at: None,
            },
            Self::Deadline { name, at } => CompletionConditionInfo {
                name: name.clone(),
                kind: CompletionConditionKind::Deadline,
                max: None,
                duration_seconds: None,
                at: Some(*at),
            },
        }
    }
}

/// Runtime evaluator for an ordered OR-set of completion conditions.
pub(super) struct CompletionTracker {
    conditions: Vec<CompletionCondition>,
    wall_started_at: DateTime<Utc>,
    monotonic_started_at: Instant,
}

impl CompletionTracker {
    pub(super) fn new(
        conditions: Vec<CompletionCondition>,
        wall_started_at: DateTime<Utc>,
        monotonic_started_at: Instant,
    ) -> Self {
        Self {
            conditions,
            wall_started_at,
            monotonic_started_at,
        }
    }

    /// Earliest active time condition. Declaration order breaks ties.
    pub(super) fn next_time_condition(&self) -> Option<(usize, Instant)> {
        self.conditions
            .iter()
            .enumerate()
            .filter_map(|(index, condition)| {
                self.time_deadline(condition)
                    .map(|deadline| (index, deadline))
            })
            .min_by_key(|(index, deadline)| (*deadline, *index))
    }

    pub(super) fn time_request(
        &self,
        index: usize,
        iteration_count: u32,
        last_signal_id: Option<SignalId>,
        requested_at: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> CompletionRequest {
        self.request(
            &self.conditions[index],
            iteration_count,
            last_signal_id,
            requested_at,
            monotonic_now,
        )
    }

    pub(super) fn due_time_request(
        &self,
        iteration_count: u32,
        last_signal_id: Option<SignalId>,
        requested_at: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> Option<CompletionRequest> {
        self.conditions
            .iter()
            .enumerate()
            .find(|(_, condition)| {
                self.time_deadline(condition)
                    .is_some_and(|deadline| monotonic_now >= deadline)
            })
            .map(|(index, _)| {
                self.time_request(
                    index,
                    iteration_count,
                    last_signal_id,
                    requested_at,
                    monotonic_now,
                )
            })
    }

    pub(super) async fn evaluate_boundary(
        &self,
        iteration_count: u32,
        last_signal_id: Option<SignalId>,
        now: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> Result<Option<CompletionRequest>, CompletionEvaluationError> {
        for condition in &self.conditions {
            let satisfied = match condition {
                CompletionCondition::Iterations { max, .. } => iteration_count >= *max,
                CompletionCondition::Elapsed { .. } | CompletionCondition::Deadline { .. } => self
                    .time_deadline(condition)
                    .is_some_and(|deadline| monotonic_now >= deadline),
                CompletionCondition::Shell {
                    name,
                    command,
                    timeout,
                    on_error,
                } => match evaluate_shell(name, command, *timeout).await {
                    Ok(satisfied) => satisfied,
                    Err(message) if *on_error == CompletionConditionErrorPolicy::Continue => {
                        tracing::warn!(
                            condition.name = %name,
                            error = %message,
                            "completion shell condition failed; keeping it pending"
                        );
                        false
                    }
                    Err(message) => {
                        return Err(CompletionEvaluationError {
                            condition_name: name.clone(),
                            message,
                        });
                    }
                },
            };
            if satisfied {
                return Ok(Some(self.request(
                    condition,
                    iteration_count,
                    last_signal_id,
                    now,
                    monotonic_now,
                )));
            }
        }
        Ok(None)
    }

    fn request(
        &self,
        condition: &CompletionCondition,
        iteration_count: u32,
        last_signal_id: Option<SignalId>,
        requested_at: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> CompletionRequest {
        CompletionRequest {
            condition: condition.info(),
            iteration_count,
            last_signal_id,
            requested_at,
            runner_started_at: self.wall_started_at,
            elapsed_seconds: monotonic_now
                .checked_duration_since(self.monotonic_started_at)
                .unwrap_or(Duration::ZERO)
                .as_secs(),
        }
    }

    fn time_deadline(&self, condition: &CompletionCondition) -> Option<Instant> {
        match condition {
            CompletionCondition::Elapsed { duration, .. } => {
                Some(self.monotonic_started_at + *duration)
            }
            CompletionCondition::Deadline { at, .. } => {
                let duration = (*at - self.wall_started_at)
                    .to_std()
                    .unwrap_or(Duration::ZERO);
                Some(self.monotonic_started_at + duration)
            }
            CompletionCondition::Iterations { .. } | CompletionCondition::Shell { .. } => None,
        }
    }
}

/// Fatal inability to evaluate a completion predicate.
#[derive(Debug, thiserror::Error)]
#[error("completion condition `{condition_name}` failed: {message}")]
pub(super) struct CompletionEvaluationError {
    condition_name: String,
    message: String,
}

impl CompletionEvaluationError {
    pub(super) fn condition_name(&self) -> &str {
        &self.condition_name
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }
}

async fn evaluate_shell(
    name: &str,
    command_source: &str,
    timeout: Duration,
) -> Result<bool, String> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(command_source)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    process_group::configure(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    // Keeps ownership of the whole predicate process tree. On timeout the
    // wait future is dropped, `kill_on_drop` reaps the shell, and the group
    // guard kills any descendants it started.
    let _group = ProcessGroup::from_child(&child);
    let status = tokio::time::timeout(timeout, child.wait())
        .await
        .map_err(|_| format!("timed out after {}s", timeout.as_secs()))?
        .map_err(|error| format!("could not wait for process: {error}"))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(code) => Err(format!("exited with status {code}")),
        None => Err(format!("terminated by a signal while evaluating `{name}`")),
    }
}
