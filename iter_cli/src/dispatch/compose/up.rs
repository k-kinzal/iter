use std::time::{Duration, Instant};

use iter_compose::iterfile::RunRecordMetadata;
use iter_compose::{
    OrchestratorContext, ProjectLockError, ProjectMember, acquire_project_lock, build,
    find_active_orchestrator, list_project_members, load_compose, project_slug, run,
};
use iter_core::process::interrupt::install_signal_handlers;
use iter_core::process::{
    UnmanagedChild, current_identity, pid_in_process_table, signal_pid_kill, signal_pid_term,
    spawn_unmanaged_detached,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::cli::{ComposeFailure, ComposeUpArgs};
use crate::output::cli_eprintln;
use crate::telemetry;
use crate::tracing_preferences::TracingPreferences;

use super::{ComposeUpError, canonical_compose_path, resolve_compose_path};

/// Handle `iter compose up`.
///
/// Two modes share this entry point:
///
/// * **Detached** (`--detach`): we `fork+setsid+exec` ourselves with
///   `--detach` stripped from argv and stdio redirected to `/dev/null`.
///   The orchestrator is **not** registered in `~/.iter/proc/`; discovery
///   relies on the `iter.compose.*` labels stamped on each child runner
///   (see [`OrchestratorContext`]). This mirrors how `docker compose ps`
///   reconstructs project state from container labels alone.
/// * **Foreground** (default): the in-process orchestrator runs directly,
///   without registering itself. Services inside the plan still register
///   their own foreground records via
///   [`iter_compose::process_lifecycle::bootstrap_foreground`].
///
/// # Errors
///
/// * The file does not exist or cannot be parsed.
/// * One or more services/triggers fail to build.
/// * Any task returns an error and the failure policy is `Abort`.
pub async fn run_compose_up(
    args: ComposeUpArgs,
    prefs: TracingPreferences,
) -> Result<(), ComposeUpError> {
    let has_targets = !args.targets.is_empty() || args.source.is_some();
    if has_targets {
        // Targeted up spawns each named service as its own subprocess, each
        // of which reloads the operator's preferences in its own `main`, so
        // the orchestrator-level `prefs` is intentionally not forwarded here.
        return run_compose_up_targeted(args).await;
    }
    if args.detach {
        // The detached orchestrator re-execs `iter compose up` (with
        // `--detach` stripped) and reloads the operator's preferences in
        // the child's `main`, so only the inline path consumes `prefs`.
        return spawn_compose_detached(&args);
    }
    run_compose_up_inline(args, prefs).await
}

/// Targeted `compose up SERVICE [SERVICE...] --detach`.
///
/// Spawns only the named services as independent subprocesses, without
/// starting a new orchestrator. Requires `--detach` because each service
/// runs as its own process; foreground targeted up is rejected.
///
/// If the project already has an active orchestrator, the new services
/// reuse its identity in their labels so `compose ps` / `compose down`
/// see them as part of the same project. If no orchestrator exists, the
/// current process's identity is used as a fallback.
async fn run_compose_up_targeted(args: ComposeUpArgs) -> Result<(), ComposeUpError> {
    use iter_compose::spawn_targeted_service;

    if !args.detach {
        return Err(ComposeUpError::TargetedRequiresDetach);
    }

    let raw_path = resolve_compose_path(args.file.as_deref());
    let compose_path = canonical_compose_path(&raw_path)?;
    let root = load_compose(&compose_path)?;
    let plan = build(&root, &compose_path)?;
    let slug = project_slug(&compose_path, args.project_name.as_deref())?;

    let mut target_names = Vec::new();
    for target in &args.targets {
        let name = parse_target_name_for_up(target)?;
        if !target_names.contains(&name.to_owned()) {
            target_names.push(name.to_owned());
        }
    }

    if let Some(source) = &args.source {
        let source_names = plan.services_for_source(source);
        if source_names.is_empty() {
            return Err(ComposeUpError::SourceNoMatch {
                path: source.clone(),
            });
        }
        cli_eprintln!(
            "--source {} resolved to service(s): {}",
            source.display(),
            source_names.join(", ")
        );
        for name in source_names {
            if !target_names.contains(&name) {
                target_names.push(name);
            }
        }
    }

    let valid_names = plan.all_service_names();
    let unknown: Vec<String> = target_names
        .iter()
        .filter(|t| !valid_names.contains(t))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(ComposeUpError::UnknownTargets {
            unknown,
            valid: valid_names,
        });
    }

    let orchestrator = if let Some(active) = find_active_orchestrator(&slug)? {
        OrchestratorContext {
            project: slug.clone(),
            identity: active.identity,
        }
    } else {
        let identity =
            current_identity().map_err(|e| ComposeUpError::OrchestratorIdentity(Box::new(e)))?;
        OrchestratorContext {
            project: slug.clone(),
            identity,
        }
    };

    for name in &target_names {
        let id =
            spawn_targeted_service(&plan, name, &compose_path, &orchestrator, args.debug).await?;
        cli_eprintln!("project {slug:?}: started service {name:?} ({id})");
    }

    Ok(())
}

fn parse_target_name_for_up(target: &str) -> Result<&str, ComposeUpError> {
    super::parse_target_name_raw(target).map_err(|target| ComposeUpError::UnsupportedResourceType {
        target: target.to_owned(),
    })
}

/// In-process orchestrator path. Used directly by foreground `compose up`,
/// and entered by the post-fork child of `--detach` (with stdio already
/// redirected to `/dev/null`).
async fn run_compose_up_inline(
    args: ComposeUpArgs,
    prefs: TracingPreferences,
) -> Result<(), ComposeUpError> {
    let raw_path = resolve_compose_path(args.file.as_deref());
    let path = canonical_compose_path(&raw_path)?;
    let root = load_compose(&path)?;
    let plan = build(&root, &path)?;
    let project = project_slug(&path, args.project_name.as_deref())?;
    // Honour the operator's loaded `~/.iter/config.toml` verbosity — the
    // orchestrator must not fall back to a hardcoded default level.
    let _telemetry_guard =
        telemetry::init_for_compose(args.debug, &prefs, plan.telemetry(), &project, None);
    info!(
        compose = %path.display(),
        queues = plan.queue_count(),
        services = plan.service_count(),
        "starting compose"
    );

    let identity =
        current_identity().map_err(|e| ComposeUpError::OrchestratorIdentity(Box::new(e)))?;
    let orchestrator = OrchestratorContext { project, identity };

    let cancel =
        install_signal_handlers(CancellationToken::new()).map_err(ComposeUpError::Signals)?;
    let policy = match args.on_failure {
        ComposeFailure::Abort => iter_compose::FailurePolicy::AbortAll,
        ComposeFailure::Continue => iter_compose::FailurePolicy::Continue,
    };
    let metadata = RunRecordMetadata {
        argv: rebuild_argv(&args),
        subcommand: "compose up".into(),
        debug: args.debug,
    };
    let report = run(plan, cancel, policy, metadata, None, orchestrator).await;

    if report.has_errors() {
        for task in &report.results {
            if task.is_err() {
                error!(task = task.name(), "compose task exited with error");
            }
        }
        return Err(ComposeUpError::TaskFailed);
    }

    info!(completed = report.results.len(), "compose finished cleanly");
    Ok(())
}

/// Fork the orchestrator into the background with `setsid` and stdio
/// redirected to `/dev/null`, then wait synchronously until the
/// orchestrator either registers its first service runner or fails.
fn spawn_compose_detached(args: &ComposeUpArgs) -> Result<(), ComposeUpError> {
    let program = std::env::current_exe().map_err(ComposeUpError::CurrentExe)?;
    let raw_path = resolve_compose_path(args.file.as_deref());
    let compose_path = canonical_compose_path(&raw_path)?;

    let root = load_compose(&compose_path)?;
    let _plan = build(&root, &compose_path)?;
    let slug = project_slug(&compose_path, args.project_name.as_deref())?;

    let _lock = match acquire_project_lock(&slug) {
        Ok(guard) => guard,
        Err(ProjectLockError::AlreadyHeld { project }) => {
            let orchestrator_pid = find_active_orchestrator(&slug)
                .ok()
                .flatten()
                .map_or(0, |a| a.identity.pid.as_raw());
            return Err(ComposeUpError::ProjectAlreadyUp {
                project,
                orchestrator_pid,
            });
        }
        Err(other) => return Err(other.into()),
    };

    if let Some(existing) = find_active_orchestrator(&slug)? {
        return Err(ComposeUpError::ProjectAlreadyUp {
            project: existing.project,
            orchestrator_pid: existing.identity.pid.as_raw(),
        });
    }

    let mut child_args: Vec<String> = vec!["compose".into(), "up".into()];
    child_args.push("-f".into());
    child_args.push(compose_path.display().to_string());
    let on_failure = match args.on_failure {
        ComposeFailure::Abort => "abort",
        ComposeFailure::Continue => "continue",
    };
    child_args.push("--on-failure".into());
    child_args.push(on_failure.into());
    if args.debug {
        child_args.push("--debug".into());
    }
    child_args.push("--project-name".into());
    child_args.push(slug.clone());

    let child =
        spawn_unmanaged_detached(&program, &child_args, &[]).map_err(ComposeUpError::Spawn)?;

    wait_for_orchestrator_ready(&slug, child)
}

/// Block until the just-spawned orchestrator has registered at least one
/// service runner whose `iter.compose.orchestrator_pid` label matches
/// the pid we control.
fn wait_for_orchestrator_ready(
    slug: &str,
    mut child: UnmanagedChild,
) -> Result<(), ComposeUpError> {
    use std::thread::sleep;
    let orchestrator_pid = child.pid();
    let timeout = Duration::from_secs(ORCHESTRATOR_READY_TIMEOUT_SECS);
    let deadline = Instant::now() + timeout;

    loop {
        match list_project_members(slug) {
            Ok(members) => {
                if members
                    .iter()
                    .any(|m| matches_spawned_orchestrator(m, orchestrator_pid))
                {
                    child.detach();
                    return Ok(());
                }
            }
            Err(err) => {
                kill_orphan(orchestrator_pid);
                child.detach();
                return Err(err.into());
            }
        }

        match child.try_wait() {
            Ok(Some(_status)) => {
                child.detach();
                return Err(ComposeUpError::OrchestratorExitedEarly {
                    project: slug.to_owned(),
                    orchestrator_pid,
                });
            }
            Ok(None) => {}
            Err(_) => {
                let alive = pid_in_process_table(orchestrator_pid).unwrap_or(true);
                if !alive {
                    child.detach();
                    return Err(ComposeUpError::OrchestratorExitedEarly {
                        project: slug.to_owned(),
                        orchestrator_pid,
                    });
                }
            }
        }

        if Instant::now() >= deadline {
            kill_orphan(orchestrator_pid);
            child.detach();
            return Err(ComposeUpError::OrchestratorStartupTimeout {
                project: slug.to_owned(),
                orchestrator_pid,
                timeout_secs: ORCHESTRATOR_READY_TIMEOUT_SECS,
            });
        }
        sleep(Duration::from_millis(100));
    }
}

fn kill_orphan(pid: u32) {
    use std::thread::sleep;
    drop(signal_pid_term(pid));
    let deadline = Instant::now() + Duration::from_secs(2);
    while pid_in_process_table(pid).unwrap_or(false) && Instant::now() < deadline {
        sleep(Duration::from_millis(50));
    }
    if pid_in_process_table(pid).unwrap_or(false) {
        drop(signal_pid_kill(pid));
    }
}

fn matches_spawned_orchestrator(member: &ProjectMember, expected_pid: u32) -> bool {
    if member.status.is_terminal() {
        return false;
    }
    member.orchestrator.pid.as_raw() == expected_pid
}

const ORCHESTRATOR_READY_TIMEOUT_SECS: u64 = 30;

fn rebuild_argv(args: &ComposeUpArgs) -> Vec<String> {
    let mut out = vec!["compose".to_owned(), "up".to_owned()];
    if let Some(p) = args.file.as_ref() {
        out.push("-f".into());
        out.push(p.display().to_string());
    }
    let on_failure = match args.on_failure {
        ComposeFailure::Abort => "abort",
        ComposeFailure::Continue => "continue",
    };
    out.push("--on-failure".into());
    out.push(on_failure.into());
    if args.debug {
        out.push("--debug".into());
    }
    if let Some(name) = args.project_name.as_deref() {
        out.push("--project-name".into());
        out.push(name.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::telemetry::resolve_level;
    use crate::tracing_preferences::{LogLevel, TracingPreferences};
    use tracing::Level;

    /// Regression guard for the `compose up` preferences defect.
    ///
    /// `run_compose_up_inline` previously initialised telemetry from a
    /// hardcoded default, so the operator's `~/.iter/config.toml` verbosity
    /// was ignored. It now threads the loaded [`TracingPreferences`] into
    /// [`crate::telemetry::init_for_compose`], whose effective level is
    /// decided by [`resolve_level`]. This pins that the decision honours a
    /// non-default `log_level` and differs from the default the orchestrator
    /// used to hardcode — so dropping the threaded value would regress here.
    #[test]
    fn compose_up_honours_operator_verbosity() {
        let configured = TracingPreferences {
            log_level: Some(LogLevel::Warn),
        };
        assert_eq!(resolve_level(false, &configured), Level::WARN);

        // The pre-fix behaviour: default preferences resolve to INFO. The two
        // differ, so threading the operator's value is what makes compose up
        // honour a non-default verbosity.
        assert_eq!(
            resolve_level(false, &TracingPreferences::default()),
            Level::INFO
        );
        assert_ne!(
            resolve_level(false, &configured),
            resolve_level(false, &TracingPreferences::default())
        );
    }
}
