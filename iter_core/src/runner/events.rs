use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::config::RunnerTerminationReason;
use super::error::ErrorSource;
use super::event::{HookEvent, SharedSignal};
use super::event_emitter::EventDispatcher;
use super::iteration::IterationContext;
use crate::agent::AgentRun;
use crate::prompt::Prompt;
use crate::runner::lifecycle::{RedactedMetadata, RunnerLifecycleEvent};
use crate::runner::observer::DynRunnerObserver;
use crate::signal::SignalId;

/// Owns the dual emission stream (system observers + user handlers) plus
/// the per-stream error tallies that surface as
/// [`RunnerSummary::event_handler_error_count`] /
/// [`RunnerSummary::observer_error_count`].
pub(super) struct RunnerEmitter {
    emitter: EventDispatcher,
    observers: Vec<Arc<dyn DynRunnerObserver>>,
    pub(super) handler_error_count: u32,
    pub(super) observer_error_count: u32,
}

impl RunnerEmitter {
    pub(super) fn new(
        emitter: EventDispatcher,
        observers: Vec<Arc<dyn DynRunnerObserver>>,
    ) -> Self {
        Self {
            emitter,
            observers,
            handler_error_count: 0,
            observer_error_count: 0,
        }
    }

    async fn observe(&mut self, lifecycle: &RunnerLifecycleEvent) {
        for (idx, obs) in self.observers.iter().enumerate() {
            if let Err(err) = obs.observe(lifecycle).await {
                self.observer_error_count = self.observer_error_count.saturating_add(1);
                tracing::warn!(
                    observer_index = idx,
                    error = %err,
                    "runner observer returned error"
                );
            }
        }
    }

    async fn emit(
        &mut self,
        event: HookEvent,
        lifecycle: Option<&RunnerLifecycleEvent>,
        snap: &IterationContext,
    ) {
        if let Some(lc) = lifecycle {
            self.observe(lc).await;
        }
        let report = self.emitter.emit(&event, snap).await;
        self.handler_error_count = self
            .handler_error_count
            .saturating_add(u32::try_from(report.error_count).unwrap_or(u32::MAX));
    }

    pub(super) async fn bootstrap(&mut self, started_at: DateTime<Utc>) {
        let lifecycle = RunnerLifecycleEvent::BootstrapStarted { started_at };
        self.observe(&lifecycle).await;
    }

    pub(super) async fn runner_starting(&mut self, snap: &IterationContext) {
        self.emit(HookEvent::RunnerStarting {}, None, snap).await;
    }

    pub(super) async fn signal_received(
        &mut self,
        signal: &SharedSignal,
        ts: DateTime<Utc>,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::SignalReceived {
            signal_id: signal.id(),
            metadata: RedactedMetadata::from_signal(signal.metadata()),
            ts,
        };
        let event = HookEvent::SignalReceived {
            signal: signal.clone(),
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn workspace_setup_starting(
        &mut self,
        signal: &SharedSignal,
        snap: &IterationContext,
    ) {
        let event = HookEvent::WorkspaceSetupStarting {
            signal: signal.clone(),
        };
        self.emit(event, None, snap).await;
    }

    pub(super) async fn workspace_setup_finished(
        &mut self,
        signal: &SharedSignal,
        path: &Path,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::WorkspaceSetup {
            signal_id: signal.id(),
            path: path.to_path_buf(),
        };
        let event = HookEvent::WorkspaceSetupFinished {
            signal: signal.clone(),
            path: path.to_path_buf(),
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn agent_starting(
        &mut self,
        signal: &SharedSignal,
        path: &Path,
        prompt: &Prompt,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::AgentStarting {
            signal_id: signal.id(),
        };
        let event = HookEvent::AgentStarting {
            signal: signal.clone(),
            path: path.to_path_buf(),
            prompt: prompt.clone(),
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn agent_finished(
        &mut self,
        signal: &SharedSignal,
        path: &Path,
        result: Result<AgentRun, String>,
        result_label: &str,
        exit: Option<i32>,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::AgentFinished {
            signal_id: signal.id(),
            result: result_label.to_owned(),
            exit,
        };
        let event = HookEvent::AgentFinished {
            signal: signal.clone(),
            path: path.to_path_buf(),
            result,
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn workspace_teardown_starting(
        &mut self,
        signal: &SharedSignal,
        path: &Path,
        snap: &IterationContext,
    ) {
        let event = HookEvent::WorkspaceTeardownStarting {
            signal: signal.clone(),
            path: path.to_path_buf(),
        };
        self.emit(event, None, snap).await;
    }

    pub(super) async fn workspace_teardown_finished(
        &mut self,
        signal: &SharedSignal,
        final_path: PathBuf,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::WorkspaceTearDown {
            signal_id: signal.id(),
        };
        let event = HookEvent::WorkspaceTeardownFinished {
            signal: signal.clone(),
            path: final_path,
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn runner_error(
        &mut self,
        error_source: ErrorSource,
        signal_id: Option<SignalId>,
        message: &str,
        event: HookEvent,
        snap: &IterationContext,
    ) {
        let lifecycle = RunnerLifecycleEvent::RunnerError {
            signal_id,
            error_source,
            error_message: message.to_owned(),
        };
        self.emit(event, Some(&lifecycle), snap).await;
    }

    pub(super) async fn runner_finished(
        &mut self,
        reason: RunnerTerminationReason,
        iteration_count: u32,
        snap: &IterationContext,
    ) {
        let event = HookEvent::RunnerFinished {
            reason,
            iteration_count,
        };
        self.emit(event, None, snap).await;
    }
}
