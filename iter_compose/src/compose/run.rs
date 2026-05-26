//! Async execution of a [`ComposePlan`]: spawn services as tasks or
//! subprocesses, join them, and produce a [`ComposeReport`].

use std::collections::BTreeMap;
use std::path::Path;

use iter_core::process::{
    DetachedSpec, ProcessHandle, ProcessId, ProcessRegistry, ProcessRuntime, ProcessStatus,
    ProcessTerminationReason, spawn_detached,
};
use iter_core::{Queue, RunnerSummary};
use iter_language::TelemetryDecl;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::error::{ServiceRunError, ServiceSubprocessError};
use super::plan::{ComposePlan, ComposeService};
use super::service::{ComposeReport, FailurePolicy, OrchestratorContext, TaskOutcome};
use super::supervisor;
use crate::process_lifecycle::{
    self, RunRecordMetadata, derive_finalize_reason, leaves_record_non_terminal,
    log_finalize_report,
};
use crate::telemetry;
use crate::trigger_argv::queue_to_url;

/// Run every service in `plan` concurrently.
///
/// Each service is registered as its own foreground process record in
/// `~/.iter/proc/<id>/` using `metadata` for the meta.json envelope —
/// which is what makes compose-managed services show up in `iter ps`,
/// `iter logs`, `iter stop`, and `iter inspect` exactly the same way an
/// `iter run` invocation does.
///
/// Each service receives a cancellation token wired to both the parent
/// `cancel` and the per-service [`ProcessRuntime`]'s shutdown
/// controller, so OS signals delivered to either layer cascade
/// correctly.
///
/// `policy` controls how the function reacts to a task error:
///
/// * [`FailurePolicy::AbortAll`] — cancel all other tasks on first error.
/// * [`FailurePolicy::Continue`] — log and let surviving tasks run on.
///
/// On return every queue declared in the plan is closed best-effort;
/// errors are logged at `warn!` level but do not affect the returned
/// [`ComposeReport`].
pub async fn run(
    plan: ComposePlan,
    cancel: CancellationToken,
    policy: FailurePolicy,
    metadata: RunRecordMetadata,
    parent_id: Option<ProcessId>,
    orchestrator: OrchestratorContext,
) -> ComposeReport {
    let ComposePlan {
        queues,
        services,
        triggers,
        telemetry,
        compose_path,
        sources: _,
    } = plan;

    let state_root = supervisor::trigger_state_root();

    let mut set: JoinSet<TaskOutcome> = JoinSet::new();

    for service in services {
        spawn_service_task(
            &mut set,
            service,
            &compose_path,
            &cancel,
            &metadata,
            parent_id,
            &orchestrator,
            telemetry.as_ref(),
        )
        .await;
    }

    for trig in triggers {
        let trigger_cancel = cancel.clone();
        let project = orchestrator.project.clone();
        let trig_name = trig.name.clone();
        let state_dir = state_root.as_ref().map(|root| {
            supervisor::trigger_state_dir(root, &project, &trig.name)
        });
        set.spawn(async move {
            let dir = state_dir.unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("iter-trigger-state")
                    .join(&project)
                    .join(&trig_name)
            });
            let outcome =
                supervisor::supervise_trigger(trig, trigger_cancel, dir).await;
            TaskOutcome::Trigger {
                name: outcome.name,
                result: outcome.result,
                final_state: outcome.status.state,
                restart_count: outcome.status.restart_count,
            }
        });
    }

    let mut outcomes = Vec::new();
    while let Some(joined) = set.join_next().await {
        let outcome = match joined {
            Ok(outcome) => outcome,
            Err(join_err) => {
                warn!(error = %join_err, "compose task panicked");
                TaskOutcome::Panic {
                    error: join_err.to_string(),
                }
            }
        };
        if outcome.is_err() && policy == FailurePolicy::AbortAll {
            cancel.cancel();
        }
        outcomes.push(outcome);
    }

    cancel.cancel();

    for (name, queue) in &queues {
        if let Err(err) = queue.close().await {
            warn!(queue = %name, error = %err, "failed to close queue cleanly");
        }
    }

    ComposeReport { outcomes }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_service_task(
    set: &mut JoinSet<TaskOutcome>,
    service: ComposeService,
    compose_path: &Path,
    cancel: &CancellationToken,
    metadata: &RunRecordMetadata,
    parent_id: Option<ProcessId>,
    orchestrator: &OrchestratorContext,
    telemetry: Option<&TelemetryDecl>,
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
            set.spawn(async move {
                let result = monitor_service_subprocess(handle, outer).await;
                TaskOutcome::ServiceSubprocess {
                    name,
                    process_id: Some(process_id),
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
            set.spawn(async move {
                let result =
                    run_one_service(service, parent_cancel, service_metadata, labels).await;
                TaskOutcome::Service { name, result }
            });
        }
        Err(ServiceSpawnDecision::Failed(name, err)) => {
            warn!(
                service = %name,
                error = %err,
                "service subprocess spawn failed; surfacing as task error",
            );
            set.spawn(async move {
                TaskOutcome::ServiceSubprocess {
                    name,
                    process_id: None,
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
    telemetry_decl: Option<&TelemetryDecl>,
) -> Result<ServiceSubprocessSpec, ServiceSpawnDecision> {
    if queue_to_url(&service.queue_decl).is_none() {
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
) -> Result<(), ServiceSubprocessError> {
    let poll = std::time::Duration::from_millis(150);
    let mut stop_sent = false;
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
                let status = handle
                    .refresh_status()
                    .await
                    .map_err(ServiceSubprocessError::Status)?;
                if status.is_terminal() {
                    return match status {
                        ProcessStatus::Stopped => Ok(()),
                        // Any external stop (targeted `compose down SERVICE`,
                        // `iter stop <id>`, or direct SIGTERM) transitions the
                        // record to `Killed` without the orchestrator requesting
                        // it. Treat this as a controlled stop so the failure
                        // policy does not cascade to sibling services.
                        ProcessStatus::Killed if !stop_sent => Ok(()),
                        other => Err(ServiceSubprocessError::NonZeroExit(other)),
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
) -> Result<RunnerSummary, ServiceRunError> {
    let runtime = process_lifecycle::bootstrap_foreground(
        &service.name,
        &service.iterfile_path,
        &metadata,
        Some(labels),
    )
    .await?;

    let outcome = run_one_service_inner(service, &parent_cancel, runtime.as_ref()).await;

    let finalize_err = if let Some(rt) = runtime {
        let failure_msg = outcome.as_ref().err().map(ToString::to_string);
        let reason = derive_finalize_reason(failure_msg, rt.shutdown());
        let report = rt.finalize(Some(reason)).await;
        log_finalize_report(&report);
        report.status_write_error.filter(leaves_record_non_terminal)
    } else {
        None
    };

    match (outcome, finalize_err) {
        (Ok(summary), None) => Ok(summary),
        (Err(runner_err), _) => Err(runner_err),
        (Ok(_), Some(finalize_err)) => Err(ServiceRunError::FinalizeStatus(finalize_err)),
    }
}

async fn run_one_service_inner(
    service: ComposeService,
    parent_cancel: &CancellationToken,
    runtime: Option<&ProcessRuntime>,
) -> Result<RunnerSummary, ServiceRunError> {
    let ComposeService {
        name,
        iterfile_path,
        queue_decl: _,
        mut builder,
    } = service;

    if let Some(rt) = runtime {
        builder = builder.observer(rt.observer().clone());
    }
    let runner = builder.build()?;

    let run_token = if let Some(rt) = runtime {
        let shutdown = rt.shutdown().clone();
        let shutdown_token = shutdown.token();
        let parent = parent_cancel.clone();
        let linker_token = shutdown_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = parent.cancelled() => {
                    shutdown.cancel(ProcessTerminationReason::SignalTerm);
                }
                () = linker_token.cancelled() => {}
            }
        });
        shutdown_token
    } else {
        parent_cancel.clone()
    };

    info!(service = %name, iterfile = %iterfile_path.display(), "starting compose service runner");

    let summary = runner.run(run_token).await?;
    info!(
        service = %name,
        iterations = summary.iteration_count,
        last = ?summary.last_signal_id,
        reason = ?summary.termination_reason,
        "compose service runner exited",
    );
    Ok(summary)
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
pub async fn spawn_targeted_service(
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

    if queue_to_url(&service.queue_decl).is_none() {
        return Err(TargetedSpawnError::NonAddressable {
            service: service_name.to_owned(),
        });
    }

    let registry =
        ProcessRegistry::open_default().map_err(TargetedSpawnError::OpenRegistry)?;

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
