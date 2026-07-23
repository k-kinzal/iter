//! Async execution of a [`ComposePlan`]: spawn services as tasks or
//! subprocesses, join them, and produce completed service state.

use std::collections::BTreeMap;
use std::path::Path;

use crate::process::{
    DetachedSpec, ProcessHandle, ProcessId, ProcessRegistry, ProcessRuntime, ProcessStatus,
    spawn_detached,
};
use iter_language::TelemetryDef;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::error::{ServiceRunError, ServiceSubprocessError};
use super::hook::{ComposeHookRuntime, ManagedEvent, ServiceTerminalState};
use super::plan::{ComposePlan, ComposeService};
use super::service::{
    CompletedServices, CompletedTask, ComposeTermination, FailurePolicy, OrchestratorContext,
};
use super::supervisor;
use crate::process_lifecycle::{
    self, ProcessTerminationReason, RunRecordMetadata, TerminationRecorder, derive_finalize_reason,
    terminal_status_for,
};
use crate::queue::queue_address;
use crate::telemetry;

/// Run every service in `plan` concurrently.
///
/// Each service is registered as its own foreground process record in
/// `~/.iter/proc/<id>/` using `metadata` for the meta.json envelope —
/// which is what makes compose-managed services show up in `iter ps`,
/// `iter logs`, `iter stop`, and `iter inspect` exactly the same way an
/// `iter run` invocation does.
///
/// Each service receives a cancellation token wired to both the parent
/// `cancel` and the per-service [`ProcessRuntime`]'s shutdown intent,
/// so OS signals delivered to either layer cascade correctly.
///
/// `policy` controls how the function reacts to a task error:
///
/// * [`FailurePolicy::AbortAll`] — cancel all other tasks on first error.
/// * [`FailurePolicy::Continue`] — log and let surviving tasks run on.
///
/// On return every queue declared in the plan is closed best-effort;
/// errors are logged at `warn!` level but do not affect the returned
pub(crate) async fn run(
    plan: ComposePlan,
    cancel: CancellationToken,
    policy: FailurePolicy,
    metadata: RunRecordMetadata,
    parent_id: Option<ProcessId>,
    orchestrator: OrchestratorContext,
) -> CompletedServices {
    let ComposePlan {
        queues,
        services,
        triggers,
        hooks: hook_plans,
        telemetry,
        compose_path,
        sources: _,
    } = plan;

    let state_root = supervisor::trigger_state_root();
    let service_names: Vec<String> = services
        .iter()
        .map(|service| service.name.clone())
        .collect();
    let trigger_names: Vec<String> = triggers
        .iter()
        .map(|trigger| trigger.name.clone())
        .collect();
    let mut hooks = ComposeHookRuntime::new(
        hook_plans,
        queues.clone(),
        orchestrator.project.clone(),
        compose_path.clone(),
        service_names,
        trigger_names,
    );
    hooks
        .compose_event(iter_language::ComposeEventName::ComposeStarting, None)
        .await;

    let mut set: JoinSet<CompletedTask> = JoinSet::new();
    let managed_cancel = CancellationToken::new();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    for service in services {
        hooks.service_starting(&service.name).await;
        spawn_service_task(
            &mut set,
            service,
            &compose_path,
            &managed_cancel,
            &metadata,
            parent_id,
            &orchestrator,
            telemetry.as_ref(),
            event_tx.clone(),
        )
        .await;
    }

    for trig in triggers {
        hooks.trigger_starting(&trig.name).await;
        let trigger_cancel = managed_cancel.clone();
        let project = orchestrator.project.clone();
        let trig_name = trig.name.clone();
        let trigger_events = event_tx.clone();
        let state_dir = state_root
            .as_ref()
            .map(|root| supervisor::trigger_state_dir(root, &project, &trig.name));
        set.spawn(async move {
            let dir = state_dir.unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("iter-trigger-state")
                    .join(&project)
                    .join(&trig_name)
            });
            let supervised =
                supervisor::supervise_trigger(trig, trigger_cancel, dir, Some(trigger_events))
                    .await;
            CompletedTask::Trigger {
                name: supervised.name,
                result: supervised.result,
                final_state: supervised.status.state,
                restart_count: supervised.status.restart_count,
            }
        });
    }
    drop(event_tx);

    let mut results = Vec::new();
    let mut compose_started = false;
    let mut external_stop = false;
    let mut fatal_error: Option<String> = None;

    while !set.is_empty() {
        tokio::select! {
            biased;
            Some(event) = event_rx.recv() => {
                apply_managed_event(&mut hooks, event).await;
                if !compose_started && hooks.all_resources_started() {
                    compose_started = true;
                    hooks
                        .compose_event(iter_language::ComposeEventName::ComposeStarted, None)
                        .await;
                }
            }
            () = cancel.cancelled(), if !external_stop => {
                external_stop = true;
                hooks
                    .compose_event(iter_language::ComposeEventName::ComposeStopping, None)
                    .await;
                managed_cancel.cancel();
            }
            joined = set.join_next() => {
                let Some(joined) = joined else {
                    break;
                };
                let completed = match joined {
                    Ok(task) => task,
                    Err(join_err) => {
                        warn!(error = %join_err, "compose task panicked");
                        CompletedTask::Panic {
                            error: join_err.to_string(),
                        }
                    }
                };

                dispatch_task_terminal(&mut hooks, &completed).await;
                if !external_stop && fatal_error.is_none() && completed.is_fatal() {
                    let message = completed
                        .fatal_message()
                        .unwrap_or_else(|| format!("managed task `{}` did not complete normally", completed.name()));
                    hooks
                        .compose_event(
                            iter_language::ComposeEventName::ComposeFailing,
                            Some(message.clone()),
                        )
                        .await;
                    fatal_error = Some(message);
                    if policy == FailurePolicy::AbortAll {
                        managed_cancel.cancel();
                    }
                }
                results.push(completed);
            }
        }
    }

    while let Ok(event) = event_rx.try_recv() {
        apply_managed_event(&mut hooks, event).await;
        if !compose_started && hooks.all_resources_started() {
            compose_started = true;
            hooks
                .compose_event(iter_language::ComposeEventName::ComposeStarted, None)
                .await;
        }
    }

    if !external_stop
        && fatal_error.is_none()
        && !results.iter().all(CompletedTask::completed_naturally)
    {
        let message =
            "all managed tasks settled, but at least one did not complete normally".to_owned();
        hooks
            .compose_event(
                iter_language::ComposeEventName::ComposeFailing,
                Some(message.clone()),
            )
            .await;
        fatal_error = Some(message);
    }

    if !external_stop && fatal_error.is_none() {
        hooks
            .compose_event(iter_language::ComposeEventName::ComposeCompleting, None)
            .await;
    }

    // Stop the OS-signal listener tasks installed around the outer token now
    // that no managed task can observe another external transition.
    cancel.cancel();
    managed_cancel.cancel();

    for (name, queue) in &queues {
        if let Err(err) = queue.close().await {
            warn!(queue = %name, error = %err, "failed to close queue cleanly");
        }
    }

    let termination = if external_stop {
        hooks
            .compose_event(iter_language::ComposeEventName::ComposeStopped, None)
            .await;
        ComposeTermination::Stopped
    } else if let Some(error) = fatal_error {
        hooks
            .compose_event(iter_language::ComposeEventName::ComposeFailed, Some(error))
            .await;
        ComposeTermination::Failed
    } else {
        hooks
            .compose_event(iter_language::ComposeEventName::ComposeCompleted, None)
            .await;
        ComposeTermination::Completed
    };

    CompletedServices {
        results,
        termination,
    }
}

async fn apply_managed_event(hooks: &mut ComposeHookRuntime, event: ManagedEvent) {
    match event {
        ManagedEvent::ServiceStarted { name } => hooks.service_started(&name).await,
        ManagedEvent::TriggerTransition {
            name,
            state,
            restart_count,
            error,
        } => {
            hooks
                .trigger_transition(&name, state, restart_count, error)
                .await;
        }
    }
}

async fn dispatch_task_terminal(hooks: &mut ComposeHookRuntime, task: &CompletedTask) {
    match task {
        CompletedTask::Service {
            name,
            state,
            result,
        } => {
            hooks
                .service_terminal(name, *state, result.as_ref().err().map(ToString::to_string))
                .await;
        }
        CompletedTask::ServiceSubprocess {
            name,
            state,
            result,
            ..
        } => {
            hooks
                .service_terminal(name, *state, result.as_ref().err().map(ToString::to_string))
                .await;
        }
        CompletedTask::Trigger { .. } | CompletedTask::Panic { .. } => {}
    }
}
async fn spawn_service_task(
    set: &mut JoinSet<CompletedTask>,
    service: ComposeService,
    compose_path: &Path,
    cancel: &CancellationToken,
    metadata: &RunRecordMetadata,
    parent_id: Option<ProcessId>,
    orchestrator: &OrchestratorContext,
    telemetry: Option<&TelemetryDef>,
    events: mpsc::UnboundedSender<ManagedEvent>,
) {
    match try_spawn_service_subprocess(
        &service,
        compose_path,
        parent_id,
        metadata.debug,
        orchestrator,
        telemetry,
    )
    .await
    {
        Ok(spawned) => {
            let ServiceSubprocessSpec {
                process_id,
                handle,
                name,
            } = spawned;
            let outer = cancel.clone();
            let monitor_events = events.clone();
            set.spawn(async move {
                let (state, result) =
                    monitor_service_subprocess(handle, outer, &name, monitor_events).await;
                CompletedTask::ServiceSubprocess {
                    name,
                    process_id: Some(process_id),
                    state,
                    result,
                }
            });
        }
        Err(ServiceSpawnDecision::Fallback(reason)) => {
            tracing::debug!(
                service = %service.name,
                reason = %reason,
                "service runs in-process (subprocess spawn not applicable)",
            );
            let parent_cancel = cancel.clone();
            let service_metadata = metadata.clone();
            let name = service.name.clone();
            let labels = orchestrator.labels_for(&name);
            let service_events = events.clone();
            set.spawn(async move {
                let (state, result) = run_one_service(
                    service,
                    parent_cancel,
                    service_metadata,
                    labels,
                    service_events,
                )
                .await;
                CompletedTask::Service {
                    name,
                    state,
                    result,
                }
            });
        }
        Err(ServiceSpawnDecision::Failed(name, err)) => {
            warn!(
                service = %name,
                error = %err,
                "service subprocess spawn failed; surfacing as task error",
            );
            set.spawn(async move {
                CompletedTask::ServiceSubprocess {
                    name,
                    process_id: None,
                    state: ServiceTerminalState::Failed,
                    result: Err(err),
                }
            });
        }
    }
}

struct ServiceSubprocessSpec {
    process_id: ProcessId,
    handle: ProcessHandle,
    name: String,
}

enum ServiceSpawnDecision {
    Fallback(String),
    Failed(String, ServiceSubprocessError),
}

async fn try_spawn_service_subprocess(
    service: &ComposeService,
    compose_path: &Path,
    parent_id: Option<ProcessId>,
    debug: bool,
    orchestrator: &OrchestratorContext,
    telemetry_decl: Option<&TelemetryDef>,
) -> Result<ServiceSubprocessSpec, ServiceSpawnDecision> {
    if !queue_address(&service.queue_decl).is_some_and(|a| a.is_addressable()) {
        return Err(ServiceSpawnDecision::Fallback(
            "queue not URL-addressable".to_string(),
        ));
    }

    let registry = match ProcessRegistry::open_default() {
        Ok(r) => r,
        Err(err) => {
            return Err(ServiceSpawnDecision::Failed(
                service.name.clone(),
                ServiceSubprocessError::OpenRegistry(err),
            ));
        }
    };

    let program = match std::env::current_exe() {
        Ok(p) => p,
        Err(err) => {
            return Err(ServiceSpawnDecision::Failed(
                service.name.clone(),
                ServiceSubprocessError::Binary(err),
            ));
        }
    };

    let args = vec![
        "run".to_string(),
        compose_path.display().to_string(),
        "--service".to_string(),
        service.name.clone(),
    ];

    let spec = DetachedSpec {
        name: service.name.clone(),
        iterfile: compose_path.to_path_buf(),
        subcommand: "run".to_string(),
        args,
        program,
        env: telemetry::service_env(telemetry_decl, &orchestrator.project, &service.name),
        debug,
        parent_id,
        labels: orchestrator.labels_for(&service.name),
    };

    let id = match spawn_detached(&registry, spec).await {
        Ok(id) => id,
        Err(err) => {
            return Err(ServiceSpawnDecision::Failed(
                service.name.clone(),
                ServiceSubprocessError::Spawn(err),
            ));
        }
    };

    let handle = match ProcessHandle::open(registry.proc_root(), id).await {
        Ok(h) => h,
        Err(err) => {
            return Err(ServiceSpawnDecision::Failed(
                service.name.clone(),
                ServiceSubprocessError::OpenHandle(err),
            ));
        }
    };

    Ok(ServiceSubprocessSpec {
        process_id: id,
        handle,
        name: service.name.clone(),
    })
}

async fn monitor_service_subprocess(
    handle: ProcessHandle,
    parent_cancel: CancellationToken,
    name: &str,
    events: mpsc::UnboundedSender<ManagedEvent>,
) -> (ServiceTerminalState, Result<(), ServiceSubprocessError>) {
    let poll = std::time::Duration::from_millis(150);
    let mut stop_sent = false;
    let mut started_sent = false;
    loop {
        tokio::select! {
            biased;
            () = parent_cancel.cancelled(), if !stop_sent => {
                if let Err(err) = handle.stop().await {
                    warn!(
                        process_id = %handle.id(),
                        error = %err,
                        "failed to forward stop to service subprocess",
                    );
                }
                stop_sent = true;
            }
            () = tokio::time::sleep(poll) => {
                let status = match handle.refresh_status().await {
                    Ok(status) => status,
                    Err(error) => {
                        return (
                            ServiceTerminalState::Failed,
                            Err(ServiceSubprocessError::Status(error)),
                        );
                    }
                };
                if status == ProcessStatus::Running && !started_sent {
                    drop(events.send(ManagedEvent::ServiceStarted {
                        name: name.to_owned(),
                    }));
                    started_sent = true;
                }
                if status.is_terminal() {
                    // `Stopped` is only reachable through `Running`; a very
                    // short service may cross both states between polls.
                    if status == ProcessStatus::Stopped && !started_sent {
                        drop(events.send(ManagedEvent::ServiceStarted {
                            name: name.to_owned(),
                        }));
                    }
                    return match status {
                        ProcessStatus::Stopped => (ServiceTerminalState::Completed, Ok(())),
                        ProcessStatus::Killed => (ServiceTerminalState::Killed, Ok(())),
                        other => (
                            ServiceTerminalState::Failed,
                            Err(ServiceSubprocessError::NonZeroExit(other)),
                        ),
                    };
                }
            }
        }
    }
}

async fn run_one_service(
    service: ComposeService,
    parent_cancel: CancellationToken,
    metadata: RunRecordMetadata,
    labels: BTreeMap<String, String>,
    events: mpsc::UnboundedSender<ManagedEvent>,
) -> (ServiceTerminalState, Result<(), ServiceRunError>) {
    let runtime = match process_lifecycle::bootstrap_foreground(
        &service.name,
        &service.iterfile_path,
        &metadata,
        Some(labels),
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return (
                ServiceTerminalState::Failed,
                Err(ServiceRunError::Lifecycle(error)),
            );
        }
    };
    let process_is_running = runtime.is_some();
    if process_is_running {
        drop(events.send(ManagedEvent::ServiceStarted {
            name: service.name.clone(),
        }));
    }

    let run_result = run_one_service_inner(
        service,
        &parent_cancel,
        runtime.as_ref(),
        &events,
        process_is_running,
    )
    .await;

    let (mut terminal_state, finalize_err) = if let Some((rt, termination)) = runtime {
        let failure_msg = run_result.as_ref().err().map(ToString::to_string);
        let reason = derive_finalize_reason(failure_msg, &termination);
        let status = terminal_status_for(&reason);
        let terminal_state = match status {
            ProcessStatus::Stopped => ServiceTerminalState::Completed,
            ProcessStatus::Killed => ServiceTerminalState::Killed,
            ProcessStatus::Initializing | ProcessStatus::Running | ProcessStatus::Failed => {
                ServiceTerminalState::Failed
            }
        };
        (terminal_state, rt.finalize(status).await.err())
    } else {
        let terminal_state = if parent_cancel.is_cancelled() {
            ServiceTerminalState::Killed
        } else if run_result.is_err() {
            ServiceTerminalState::Failed
        } else {
            ServiceTerminalState::Completed
        };
        (terminal_state, None)
    };

    let result = match (run_result, finalize_err) {
        (Ok(()), None) => Ok(()),
        (Err(runner_err), _) => Err(runner_err),
        (Ok(_), Some(finalize_err)) => Err(ServiceRunError::FinalizeStatus(finalize_err)),
    };
    if result.is_err() && !matches!(terminal_state, ServiceTerminalState::Killed) {
        terminal_state = ServiceTerminalState::Failed;
    }
    (terminal_state, result)
}

async fn run_one_service_inner(
    service: ComposeService,
    parent_cancel: &CancellationToken,
    runtime: Option<&(ProcessRuntime, TerminationRecorder)>,
    events: &mpsc::UnboundedSender<ManagedEvent>,
    started_sent: bool,
) -> Result<(), ServiceRunError> {
    let ComposeService {
        name,
        iterfile_path,
        queue_decl: _,
        mut builder,
    } = service;

    if let Some((rt, _)) = runtime {
        builder = crate::start::wire_builder_runtime(builder, rt);
    }
    let runner = builder.build()?;
    if !started_sent {
        drop(events.send(ManagedEvent::ServiceStarted { name: name.clone() }));
    }

    let run_token = if let Some((rt, termination)) = runtime {
        let termination = termination.clone();
        let shutdown_token = rt.shutdown().token();
        let parent = parent_cancel.clone();
        let linker_token = shutdown_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = parent.cancelled() => {
                    // Every parent-orchestrated stop — compose `down`,
                    // AbortAll after a sibling failure, orchestrator
                    // shutdown — is deliberately classified as SignalTerm:
                    // the record reads `Killed`, matching what the operator
                    // observes for any externally initiated stop.
                    termination.cancel(ProcessTerminationReason::SignalTerm);
                }
                () = linker_token.cancelled() => {}
            }
        });
        shutdown_token
    } else {
        parent_cancel.clone()
    };

    info!(service = %name, iterfile = %iterfile_path.display(), "starting compose service runner");

    runner.run(run_token).await?;
    Ok(())
}

/// Spawn a single named service from a built compose plan as a detached
/// subprocess.
///
/// Used by targeted `compose up SERVICE --detach` to start individual
/// services without a full orchestrator. The service must use a
/// URL-addressable queue so the subprocess can connect to it
/// independently.
///
/// Returns the allocated [`ProcessId`] on success. The subprocess
/// runs `iter run <compose_path> --service <name>` and registers in
/// `~/.iter/proc/` with the same labels a full `compose up` would
/// stamp.
///
/// # Errors
///
/// * The named service does not exist in the plan.
/// * The service's queue is not URL-addressable.
/// * Opening the process registry, locating the binary, or spawning
///   the child fails.
pub(crate) async fn spawn_targeted_service(
    plan: &ComposePlan,
    service_name: &str,
    compose_path: &Path,
    orchestrator: &OrchestratorContext,
    debug: bool,
) -> Result<ProcessId, super::error::TargetedSpawnError> {
    use super::error::TargetedSpawnError;

    let service = plan
        .services
        .iter()
        .find(|s| s.name == service_name)
        .ok_or_else(|| TargetedSpawnError::UnknownService(service_name.to_owned()))?;

    if !queue_address(&service.queue_decl).is_some_and(|a| a.is_addressable()) {
        return Err(TargetedSpawnError::NonAddressable {
            service: service_name.to_owned(),
        });
    }

    let registry = ProcessRegistry::open_default().map_err(TargetedSpawnError::OpenRegistry)?;

    let program = std::env::current_exe().map_err(TargetedSpawnError::Binary)?;

    let args = vec![
        "run".to_string(),
        compose_path.display().to_string(),
        "--service".to_string(),
        service.name.clone(),
    ];

    let spec = DetachedSpec {
        name: service.name.clone(),
        iterfile: compose_path.to_path_buf(),
        subcommand: "run".to_string(),
        args,
        program,
        env: telemetry::service_env(
            plan.telemetry.as_ref(),
            &orchestrator.project,
            &service.name,
        ),
        debug,
        parent_id: None,
        labels: orchestrator.labels_for(&service.name),
    };

    let id = spawn_detached(&registry, spec)
        .await
        .map_err(TargetedSpawnError::Spawn)?;

    Ok(id)
}
