//! Trigger supervisor: restart loop, backoff, and lifecycle state
//! persistence for compose-managed triggers.
//!
//! Long-running triggers (`watch`, `cron`, `command`, `webhook`) are
//! restarted automatically when they exit unexpectedly or return an
//! error.  Finite triggers (`files` without `no_exit_on_eof`) may
//! complete normally without restart.
//!
//! Lifecycle state is persisted to per-trigger JSON files under
//! `~/.iter/trigger-state/<project>/<trigger>/` so `iter compose ps`
//! and `iter inspect` can report trigger health without relying on
//! logs alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::trigger::{ComposeTrigger, TriggerRunError, enqueue_terminate, run_trigger_once};

const MAX_BACKOFF: Duration = Duration::from_secs(60);
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Lifecycle state of a supervised trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TriggerLifecycleState {
    /// Initial state before the trigger's first run.
    Starting,
    /// The trigger is actively executing.
    Running,
    /// The trigger exited and the supervisor is waiting before relaunching.
    Restarting,
    /// A build-time error prevented the trigger from starting.
    Failed,
    /// The orchestrator was shut down.
    Stopped,
    /// A finite trigger finished normally.
    Completed,
}

impl std::fmt::Display for TriggerLifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Restarting => "Restarting",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
        };
        f.write_str(s)
    }
}

/// Persisted status snapshot for a supervised trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TriggerStatus {
    /// Trigger name as declared in the compose file.
    pub(crate) name: String,
    /// Current lifecycle state.
    pub(crate) state: TriggerLifecycleState,
    /// Trigger kind (e.g. `"cron"`, `"watch"`, `"files"`).
    pub(crate) kind: String,
    /// Number of supervisor-initiated restarts since the orchestrator booted.
    pub(crate) restart_count: u32,
    /// Human-readable description of the most recent error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_error: Option<String>,
    /// Wall-clock time of the most recent state transition.
    pub(crate) last_state_change: DateTime<Utc>,
    /// Whether this trigger may complete normally without restart.
    pub(crate) is_finite: bool,
}

/// Result of a supervised trigger run, returned as part of
/// [`super::service::CompletedTask::Trigger`].
#[derive(Debug)]
pub(crate) struct TriggerSupervisorRun {
    pub(crate) name: String,
    pub(crate) status: TriggerStatus,
    pub(crate) result: Result<(), TriggerRunError>,
}

/// Root directory for trigger state, sibling to `~/.iter/proc/`.
#[must_use]
pub(crate) fn trigger_state_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".iter").join("trigger-state"))
}

/// Per-trigger state directory.
#[must_use]
pub(crate) fn trigger_state_dir(base: &Path, project: &str, trigger_name: &str) -> PathBuf {
    base.join(project).join(trigger_name)
}

fn write_status(dir: &Path, status: &TriggerStatus) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        warn!(
            trigger = %status.name,
            error = %e,
            "failed to create trigger state directory",
        );
        return;
    }
    let path = dir.join("status.json");
    let tmp = dir.join("status.json.tmp");
    match serde_json::to_string_pretty(status) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&tmp, &json) {
                warn!(
                    trigger = %status.name,
                    error = %e,
                    "failed to write trigger status",
                );
                return;
            }
            if let Err(e) = std::fs::rename(&tmp, &path) {
                warn!(
                    trigger = %status.name,
                    error = %e,
                    "failed to rename trigger status file",
                );
            }
        }
        Err(e) => {
            warn!(
                trigger = %status.name,
                error = %e,
                "failed to serialize trigger status",
            );
        }
    }
}

/// Read a previously-persisted trigger status from disk.
#[must_use]
pub(crate) fn read_status(dir: &Path) -> Option<TriggerStatus> {
    let path = dir.join("status.json");
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

fn compute_backoff(restart_count: u32) -> Duration {
    let secs = BASE_BACKOFF
        .as_secs()
        .saturating_mul(2u64.saturating_pow(restart_count.min(6)));
    Duration::from_secs(secs.min(MAX_BACKOFF.as_secs()))
}

fn make_result(
    name: String,
    status: TriggerStatus,
    result: Result<(), TriggerRunError>,
) -> TriggerSupervisorRun {
    TriggerSupervisorRun {
        name,
        status,
        result,
    }
}

fn transition(status: &mut TriggerStatus, state: TriggerLifecycleState, state_dir: &Path) {
    status.state = state;
    status.last_state_change = Utc::now();
    write_status(state_dir, status);
}

/// Supervise a trigger for the lifetime of the orchestrator.
///
/// Long-running triggers are restarted after unexpected exit or error.
/// Finite triggers are allowed to complete normally.  Build errors are
/// not retried.
///
/// The supervisor writes lifecycle state to `state_dir` on every
/// transition so external tooling can inspect trigger health.
pub(crate) async fn supervise_trigger(
    trigger: ComposeTrigger,
    cancel: CancellationToken,
    state_dir: PathBuf,
) -> TriggerSupervisorRun {
    let name = trigger.name.clone();
    let finite = trigger.is_finite();
    let kind = trigger.kind_name().to_owned();
    let terminate_on_completion = trigger.terminate_on_completion;

    let mut trigger = trigger;
    trigger.state_dir = Some(state_dir.clone());

    let mut status = TriggerStatus {
        name: name.clone(),
        state: TriggerLifecycleState::Starting,
        kind,
        restart_count: 0,
        last_error: None,
        last_state_change: Utc::now(),
        is_finite: finite,
    };
    write_status(&state_dir, &status);

    loop {
        info!(trigger = %name, restart_count = status.restart_count, "starting compose trigger");
        transition(&mut status, TriggerLifecycleState::Running, &state_dir);

        let result = run_trigger_once(&trigger, cancel.clone()).await;

        if cancel.is_cancelled() {
            if let Err(ref e) = result {
                status.last_error = Some(e.to_string());
            }
            transition(&mut status, TriggerLifecycleState::Stopped, &state_dir);
            return make_result(name, status, result);
        }

        match &result {
            Ok(()) if finite => {
                info!(trigger = %name, "finite trigger completed normally");
                transition(&mut status, TriggerLifecycleState::Completed, &state_dir);
                if terminate_on_completion {
                    if let Err(e) = enqueue_terminate(&trigger).await {
                        warn!(trigger = %name, error = %e, "failed to enqueue terminate signal");
                        status.last_error = Some(e.to_string());
                        write_status(&state_dir, &status);
                        return make_result(name, status, Err(e));
                    }
                }
                return make_result(name, status, Ok(()));
            }
            Ok(()) => {
                warn!(trigger = %name, "long-running trigger exited unexpectedly; will restart");
                status.last_error = Some("exited unexpectedly".to_owned());
                write_status(&state_dir, &status);
            }
            Err(e) if matches!(e, TriggerRunError::Build(_)) => {
                warn!(trigger = %name, error = %e, "trigger build failed; will not restart");
                status.last_error = Some(e.to_string());
                transition(&mut status, TriggerLifecycleState::Failed, &state_dir);
                return make_result(name, status, result);
            }
            Err(e) => {
                warn!(trigger = %name, error = %e, "trigger failed; will restart");
                status.last_error = Some(e.to_string());
                write_status(&state_dir, &status);
            }
        }

        drop(result);
        status.restart_count += 1;
        let backoff = compute_backoff(status.restart_count);
        transition(&mut status, TriggerLifecycleState::Restarting, &state_dir);
        let backoff_ms = backoff.as_millis() as u64;
        warn!(trigger = %name, backoff_ms, restart_count = status.restart_count, "waiting before restart");

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                transition(&mut status, TriggerLifecycleState::Stopped, &state_dir);
                return make_result(name, status, Ok(()));
            }
            () = tokio::time::sleep(backoff) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use iter_core::Queue;
    use iter_core::queue::InMemoryQueue;
    use iter_language::TriggerDef;

    fn make_trigger(name: &str, decl: TriggerDef) -> ComposeTrigger {
        let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
        ComposeTrigger {
            name: name.to_string(),
            decl,
            queue,
            terminate_on_completion: false,
            state_dir: None,
        }
    }

    fn finite_files_decl(path: &str) -> TriggerDef {
        TriggerDef::Files {
            sources: vec![iter_language::FilesSource::Path(path.to_string())],
            no_exit_on_eof: false,
            base_metadata: vec![],
            priority: None,
            max_signals: None,
        }
    }

    #[test]
    fn backoff_is_bounded() {
        // At runtime restart_count is incremented before calling compute_backoff,
        // so the first actual backoff is compute_backoff(1) = 2s.
        assert_eq!(compute_backoff(0), Duration::from_secs(1));
        assert_eq!(compute_backoff(1), Duration::from_secs(2));
        assert_eq!(compute_backoff(2), Duration::from_secs(4));
        assert_eq!(compute_backoff(3), Duration::from_secs(8));
        assert_eq!(compute_backoff(6), Duration::from_secs(64).min(MAX_BACKOFF));
        assert_eq!(compute_backoff(100), MAX_BACKOFF);
    }

    #[test]
    fn lifecycle_state_display() {
        assert_eq!(TriggerLifecycleState::Running.to_string(), "Running");
        assert_eq!(TriggerLifecycleState::Restarting.to_string(), "Restarting");
        assert_eq!(TriggerLifecycleState::Stopped.to_string(), "Stopped");
    }

    #[test]
    fn status_round_trips_through_json() {
        let status = TriggerStatus {
            name: "my_watch".into(),
            state: TriggerLifecycleState::Running,
            kind: "watch".into(),
            restart_count: 2,
            last_error: Some("io error".into()),
            last_state_change: Utc::now(),
            is_finite: false,
        };
        let json = serde_json::to_string(&status).unwrap();
        let recovered: TriggerStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.name, "my_watch");
        assert_eq!(recovered.state, TriggerLifecycleState::Running);
        assert_eq!(recovered.restart_count, 2);
    }

    #[test]
    fn write_and_read_status_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let status = TriggerStatus {
            name: "test_trigger".into(),
            state: TriggerLifecycleState::Completed,
            kind: "files".into(),
            restart_count: 0,
            last_error: None,
            last_state_change: Utc::now(),
            is_finite: true,
        };
        write_status(dir.path(), &status);
        let recovered = read_status(dir.path()).expect("should read back");
        assert_eq!(recovered.name, "test_trigger");
        assert_eq!(recovered.state, TriggerLifecycleState::Completed);
        assert!(recovered.is_finite);
    }

    #[test]
    fn is_finite_detects_files_without_no_exit_on_eof() {
        let finite = make_trigger("f", finite_files_decl("/dev/null"));
        assert!(finite.is_finite());

        let infinite = make_trigger(
            "inf",
            TriggerDef::Files {
                sources: vec![],
                no_exit_on_eof: true,
                base_metadata: vec![],
                priority: None,
                max_signals: None,
            },
        );
        assert!(!infinite.is_finite());

        let cron = make_trigger(
            "c",
            TriggerDef::Cron {
                schedule: "* * * * *".into(),
                timezone: None,
                at_startup: false,
                catch_up_secs: None,
                jitter_secs: None,
                base_metadata: vec![],
                priority: None,
                max_signals: None,
            },
        );
        assert!(!cron.is_finite());
    }

    #[tokio::test]
    async fn finite_trigger_completes_normally() {
        let dir = tempfile::tempdir().unwrap();
        let input_file = dir.path().join("input.txt");
        std::fs::write(&input_file, "line1\nline2\n").unwrap();

        let state_dir = dir.path().join("state");
        let trigger = make_trigger(
            "finite_test",
            finite_files_decl(input_file.to_str().unwrap()),
        );

        let cancel = CancellationToken::new();
        let supervised = supervise_trigger(trigger, cancel, state_dir.clone()).await;

        assert_eq!(supervised.status.state, TriggerLifecycleState::Completed);
        assert_eq!(supervised.status.restart_count, 0);
        assert!(supervised.result.is_ok());

        let persisted = read_status(&state_dir).unwrap();
        assert_eq!(persisted.state, TriggerLifecycleState::Completed);
    }

    #[tokio::test]
    async fn terminate_on_completion_fires_for_finite_trigger() {
        let dir = tempfile::tempdir().unwrap();
        let input_file = dir.path().join("input.txt");
        std::fs::write(&input_file, "line1\n").unwrap();

        let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
        let trigger = ComposeTrigger {
            name: "term_test".to_string(),
            decl: finite_files_decl(input_file.to_str().unwrap()),
            queue: queue.clone(),
            terminate_on_completion: true,
            state_dir: None,
        };

        let cancel = CancellationToken::new();
        let state_dir = dir.path().join("state");
        let supervised = supervise_trigger(trigger, cancel, state_dir).await;

        assert_eq!(supervised.status.state, TriggerLifecycleState::Completed);
        assert!(supervised.result.is_ok());

        // Verify terminate signal was enqueued
        let dq_cancel = CancellationToken::new();
        queue.close().await.unwrap();
        let mut found_terminate = false;
        while let Ok(Some(signal)) = queue.dequeue(dq_cancel.clone()).await {
            if signal.is_terminate() {
                found_terminate = true;
            }
        }
        assert!(
            found_terminate,
            "terminate signal should be enqueued after finite completion"
        );
    }

    #[tokio::test]
    async fn supervisor_stops_on_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");

        let trigger = make_trigger(
            "cancel_test",
            TriggerDef::Cron {
                schedule: "0 0 1 1 *".into(),
                timezone: None,
                at_startup: false,
                catch_up_secs: None,
                jitter_secs: None,
                base_metadata: vec![],
                priority: None,
                max_signals: None,
            },
        );

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_clone.cancel();
        });

        let supervised = supervise_trigger(trigger, cancel, state_dir.clone()).await;

        assert_eq!(supervised.status.state, TriggerLifecycleState::Stopped);

        let persisted = read_status(&state_dir).unwrap();
        assert_eq!(persisted.state, TriggerLifecycleState::Stopped);
    }

    #[tokio::test]
    async fn run_error_triggers_restart_with_persisted_status() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");

        let trigger = ComposeTrigger {
            name: "restart_test".to_string(),
            decl: finite_files_decl("/nonexistent/error_path.txt"),
            queue: Arc::new(InMemoryQueue::new()),
            terminate_on_completion: false,
            state_dir: None,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            cancel_clone.cancel();
        });

        let supervised = supervise_trigger(trigger, cancel, state_dir.clone()).await;

        assert_eq!(supervised.status.state, TriggerLifecycleState::Stopped);
        assert!(
            supervised.status.restart_count >= 1,
            "expected at least 1 restart, got {}",
            supervised.status.restart_count
        );
        assert!(
            supervised.status.last_error.is_some(),
            "last_error should record the failure message"
        );

        let persisted = read_status(&state_dir).unwrap();
        assert_eq!(persisted.state, TriggerLifecycleState::Stopped);
        assert_eq!(persisted.restart_count, supervised.status.restart_count);
        assert!(persisted.last_error.is_some());
    }

    #[tokio::test]
    async fn terminate_not_fired_on_supervised_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");

        // This file doesn't exist, so opening it will error. The error is a
        // Run error (not Build), so the supervisor will restart.
        let trigger = ComposeTrigger {
            name: "restart_no_term".to_string(),
            decl: finite_files_decl("/nonexistent/path/to/input.txt"),
            queue: Arc::new(InMemoryQueue::new()),
            terminate_on_completion: true,
            state_dir: None,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            // Let it restart once, then cancel during backoff
            tokio::time::sleep(Duration::from_millis(1500)).await;
            cancel_clone.cancel();
        });

        let supervised = supervise_trigger(trigger, cancel, state_dir).await;

        // Should be stopped (cancelled during backoff), not completed
        assert_eq!(supervised.status.state, TriggerLifecycleState::Stopped);
        // Should have attempted at least one restart
        assert!(
            supervised.status.restart_count >= 1,
            "expected at least 1 restart, got {}",
            supervised.status.restart_count
        );
    }
}
