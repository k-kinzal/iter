//! Compose service types: orchestrator context, failure policy, completed
//! tasks, and the completed-service aggregate.

use std::collections::BTreeMap;

use crate::process::{ProcessId, ProcessIdentity};

use super::error::{ServiceRunError, ServiceSubprocessError};
use super::hook::ServiceTerminalState;
use super::supervisor::TriggerLifecycleState;
use super::trigger::TriggerRunError;

/// How [`super::run`] reacts to the first failing service.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FailurePolicy {
    /// Cancel every other task on the first error.
    #[default]
    AbortAll,
    /// Log the failure and let the surviving tasks run to completion.
    Continue,
}

/// Result of a single spawned service task.
#[derive(Debug)]
pub(crate) enum CompletedTask {
    /// A service runner completed (or errored).
    Service {
        /// Service name from the compose file.
        name: String,
        /// Terminal iter-process classification observed by Compose.
        state: ServiceTerminalState,
        /// `Ok(())` if the runner completed cleanly, `Err(_)` if it
        /// failed to bootstrap, build, run, or finalize.
        result: Result<(), ServiceRunError>,
    },
    /// A subprocess-spawned service completed (or errored). Used when
    /// the service's queue has a cross-process URL form (file://, redis://);
    /// non-addressable queues fall through to [`CompletedTask::Service`]
    /// (in-process) instead.
    ServiceSubprocess {
        /// Service name from the compose file.
        name: String,
        /// Allocated process id of the child registry record. `None`
        /// when the spawn never succeeded.
        process_id: Option<ProcessId>,
        /// Terminal iter-process classification observed by Compose.
        state: ServiceTerminalState,
        /// `Ok(())` if the child exited cleanly, `Err(_)` otherwise.
        result: Result<(), ServiceSubprocessError>,
    },
    /// A supervised trigger task completed, stopped, or failed.
    Trigger {
        /// Trigger name from the compose file.
        name: String,
        /// `Ok(())` if the trigger completed or stopped cleanly,
        /// `Err(_)` on build or unrecoverable runtime failure.
        result: Result<(), TriggerRunError>,
        /// Final lifecycle state reported by the supervisor.
        final_state: TriggerLifecycleState,
        /// Number of supervisor-initiated restarts.
        restart_count: u32,
    },
    /// A spawned task panicked (or was cancelled by `JoinSet`
    /// teardown). Synthesised in [`super::run`]'s join loop so panics
    /// surface in [`CompletedServices::has_errors`] and trigger
    /// [`FailurePolicy::AbortAll`]; without this synthesis a panic
    /// was silently dropped and the abort policy was defeated (Codex
    /// iter-9 Major 3).
    Panic {
        /// `JoinError` description; we cannot recover the task's own
        /// name once the panic propagates, so the message is the only
        /// identifier we have.
        error: String,
    },
}

impl CompletedTask {
    /// `true` when this task did not complete naturally.
    #[must_use]
    pub(crate) fn is_err(&self) -> bool {
        self.is_fatal()
    }

    /// `true` when the task prevents natural Compose completion.
    #[must_use]
    pub(crate) fn is_fatal(&self) -> bool {
        match self {
            Self::Service { state, .. } | Self::ServiceSubprocess { state, .. } => {
                matches!(state, ServiceTerminalState::Failed)
            }
            Self::Trigger {
                final_state,
                result,
                ..
            } => result.is_err() || !matches!(final_state, TriggerLifecycleState::Completed),
            Self::Panic { .. } => true,
        }
    }

    /// `true` when this managed task reached its normal terminal state.
    #[must_use]
    pub(crate) fn completed_naturally(&self) -> bool {
        match self {
            Self::Service { state, result, .. } => {
                matches!(state, ServiceTerminalState::Completed) && result.is_ok()
            }
            Self::ServiceSubprocess { state, result, .. } => {
                matches!(state, ServiceTerminalState::Completed) && result.is_ok()
            }
            Self::Trigger {
                final_state,
                result,
                ..
            } => matches!(final_state, TriggerLifecycleState::Completed) && result.is_ok(),
            Self::Panic { .. } => false,
        }
    }

    /// Human-readable detail for the first fatal Compose transition.
    #[must_use]
    pub(crate) fn fatal_message(&self) -> Option<String> {
        match self {
            Self::Service { name, result, .. } => match result {
                Err(error) => Some(format!("service `{name}` failed: {error}")),
                Ok(()) => None,
            },
            Self::ServiceSubprocess { name, result, .. } => match result {
                Err(error) => Some(format!("service `{name}` failed: {error}")),
                Ok(()) => None,
            },
            Self::Trigger {
                name,
                result,
                final_state,
                ..
            } => match result {
                Err(error) => Some(format!("Trigger `{name}` failed: {error}")),
                Ok(()) if !matches!(final_state, TriggerLifecycleState::Completed) => {
                    Some(format!("Trigger `{name}` stopped in state {final_state}"))
                }
                Ok(()) => None,
            },
            Self::Panic { error } => Some(format!("managed task panicked: {error}")),
        }
    }

    /// Display name of the underlying service or trigger.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Service { name, .. }
            | Self::ServiceSubprocess { name, .. }
            | Self::Trigger { name, .. } => name.as_str(),
            Self::Panic { .. } => "<panicked task>",
        }
    }
}

/// Terminal classification of one Compose orchestrator run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ComposeTermination {
    /// Every managed task completed normally.
    #[default]
    Completed,
    /// At least one managed task did not complete normally.
    Failed,
    /// The operator externally stopped the Compose run.
    Stopped,
}

/// Completed service and trigger tasks returned by [`super::run`].
#[derive(Debug, Default)]
pub(crate) struct CompletedServices {
    /// One entry per spawned service task, in completion order.
    pub(crate) results: Vec<CompletedTask>,
    /// Mutually exclusive Compose terminal classification.
    pub(crate) termination: ComposeTermination,
}

impl CompletedServices {
    /// `true` when at least one result carries an error.
    #[must_use]
    pub(crate) fn has_errors(&self) -> bool {
        self.termination == ComposeTermination::Failed
    }
}

/// `ProcessMetadata.labels` key for the docker-compose-style project slug
/// stamped onto every service runner the orchestrator spawns.
pub(crate) const LABEL_PROJECT: &str = "iter.compose.project";
/// `ProcessMetadata.labels` key naming the compose service that owns this
/// runner.
pub(crate) const LABEL_SERVICE: &str = "iter.compose.service";
/// `ProcessMetadata.labels` key recording the orchestrator's pid so
/// `iter compose down` can locate it without scanning the process table.
pub(crate) const LABEL_ORCHESTRATOR_PID: &str = "iter.compose.orchestrator_pid";
/// `ProcessMetadata.labels` key recording the orchestrator's
/// [`ProcessStartTime`] fingerprint (round-trip form). Combined with
/// [`LABEL_ORCHESTRATOR_PID`] this is reuse-proof against pid recycling.
///
/// [`ProcessStartTime`]: crate::process::ProcessStartTime
pub(crate) const LABEL_ORCHESTRATOR_START_TIME: &str = "iter.compose.orchestrator_start_time";
/// `ProcessMetadata.labels` key recording the orchestrator's Linux
/// `boot_id` (`/proc/sys/kernel/random/boot_id`). Present only on Linux;
/// absent on macOS where the kernel start-time is reuse-proof on its own.
/// Required by [`process_is_alive_with_start_time`] to reject the case
/// where pid + tick-since-boot collide across reboots.
///
/// [`process_is_alive_with_start_time`]: crate::process::process_is_alive_with_start_time
pub(crate) const LABEL_ORCHESTRATOR_BOOT_ID: &str = "iter.compose.orchestrator_boot_id";

/// Per-orchestrator identity passed into [`super::run`].
///
/// Captured once at orchestrator start and stamped onto every service
/// runner via `meta.json` labels, so `iter compose ls`/`ps`/`down` can
/// reconstruct the project graph from runner state alone — the same
/// label-discovery model docker compose uses on container labels.
#[derive(Clone)]
pub(crate) struct OrchestratorContext {
    /// Docker-compose-style project slug (see [`crate::project::project_slug`]).
    pub(crate) project: String,
    /// Identity of the orchestrator process itself (pid + start-time
    /// fingerprint). Used to populate [`LABEL_ORCHESTRATOR_PID`] /
    /// [`LABEL_ORCHESTRATOR_START_TIME`].
    pub(crate) identity: ProcessIdentity,
}

impl OrchestratorContext {
    /// Build the per-service label map written into `meta.json` for a
    /// runner the orchestrator spawned.
    pub(crate) fn labels_for(&self, service: &str) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_PROJECT.to_string(), self.project.clone());
        labels.insert(LABEL_SERVICE.to_string(), service.to_string());
        labels.insert(
            LABEL_ORCHESTRATOR_PID.to_string(),
            self.identity.pid.as_raw().to_string(),
        );
        labels.insert(
            LABEL_ORCHESTRATOR_START_TIME.to_string(),
            self.identity.start_time.to_label_string(),
        );
        if let Some(boot) = self.identity.linux_boot_id.as_deref() {
            labels.insert(LABEL_ORCHESTRATOR_BOOT_ID.to_string(), boot.to_string());
        }
        labels
    }
}

impl std::fmt::Debug for OrchestratorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorContext")
            .field("project", &self.project)
            .field("pid", &self.identity.pid.as_raw())
            .field("start_time", &self.identity.start_time.to_label_string())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Pid, ProcessStartTime};

    #[test]
    fn labels_for_round_trips_orchestrator_identity() {
        let identity = ProcessIdentity {
            pid: Pid::new(12345),
            start_time: ProcessStartTime::LinuxClockTicks(987_654),
            linux_boot_id: Some("11111111-2222-3333-4444-555555555555".into()),
        };
        let ctx = OrchestratorContext {
            project: "demo".into(),
            identity: identity.clone(),
        };
        let labels = ctx.labels_for("worker");
        assert_eq!(labels.get(LABEL_PROJECT).unwrap(), "demo");
        assert_eq!(labels.get(LABEL_SERVICE).unwrap(), "worker");
        assert_eq!(labels.get(LABEL_ORCHESTRATOR_PID).unwrap(), "12345");
        assert_eq!(
            labels.get(LABEL_ORCHESTRATOR_START_TIME).unwrap(),
            &identity.start_time.to_label_string()
        );
        assert_eq!(
            labels.get(LABEL_ORCHESTRATOR_BOOT_ID).unwrap(),
            "11111111-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn labels_for_omits_boot_id_on_macos() {
        let identity = ProcessIdentity {
            pid: Pid::new(7),
            start_time: ProcessStartTime::MacosEpochMicros(1_700_000_000_000_000),
            linux_boot_id: None,
        };
        let ctx = OrchestratorContext {
            project: "demo".into(),
            identity,
        };
        let labels = ctx.labels_for("worker");
        assert!(
            !labels.contains_key(LABEL_ORCHESTRATOR_BOOT_ID),
            "macos labels must not carry boot_id; got: {labels:?}"
        );
    }
}
