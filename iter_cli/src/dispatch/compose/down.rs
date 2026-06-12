use std::time::{Duration, Instant};

use crate::{ProjectMember, build, find_active_orchestrator, list_project_members, load_compose};
use iter_core::process::{
    PidFileState, PosixSignal, ProcessError, ProcessHandle, ProcessRegistry,
    process_is_alive_with_start_time, signal_identity,
};
use tokio::time::sleep;

use crate::cli::ComposeDownArgs;
use crate::output::{cli_eprintln, trunc_id};

use super::{
    ComposeRuntimeError, canonicalize_compose_path, resolve_compose_path, runtime_project_slug,
};

fn parse_target_name(target: &str) -> Result<&str, ComposeRuntimeError> {
    super::parse_target_name_raw(target).map_err(|target| {
        ComposeRuntimeError::UnsupportedResourceType {
            target: target.to_owned(),
        }
    })
}

/// Resolve positional targets and `--source` into a deduplicated list of
/// service names. Returns `None` when no selectors were given (project-wide).
fn resolve_down_targets(
    args: &ComposeDownArgs,
) -> Result<Option<Vec<String>>, ComposeRuntimeError> {
    let has_positional = !args.targets.is_empty();
    let has_source = args.source.is_some();
    if !has_positional && !has_source {
        return Ok(None);
    }

    let mut names: Vec<String> = Vec::new();

    for target in &args.targets {
        let name = parse_target_name(target)?;
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }

    if let Some(source) = &args.source {
        let raw_compose = resolve_compose_path(args.file.as_deref());
        let compose_path =
            canonicalize_compose_path(&raw_compose).unwrap_or_else(|_| raw_compose.clone());
        let root = load_compose(&compose_path)?;
        let plan = build(&root, &compose_path)?;
        let source_names = plan.services_for_source(source);
        if source_names.is_empty() {
            return Err(ComposeRuntimeError::SourceNoMatch {
                path: source.clone(),
            });
        }
        if !args.quiet {
            cli_eprintln!(
                "--source {} resolved to service(s): {}",
                source.display(),
                source_names.join(", ")
            );
        }
        for name in source_names {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }

    Ok(Some(names))
}

fn validate_targets_against_members(
    targets: &[String],
    members: &[ProjectMember],
) -> Result<(), ComposeRuntimeError> {
    use std::collections::BTreeSet;
    let known: BTreeSet<&str> = members
        .iter()
        .filter(|m| !m.service.is_empty())
        .map(|m| m.service.as_str())
        .collect();
    let unknown: Vec<String> = targets
        .iter()
        .filter(|t| !known.contains(t.as_str()))
        .cloned()
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        let live: Vec<String> = members
            .iter()
            .filter(|m| !m.service.is_empty() && !m.status.is_terminal())
            .map(|m| m.service.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Err(ComposeRuntimeError::UnknownTargets {
            unknown,
            valid: live,
        })
    }
}

/// Handle `iter compose down`. Mirrors `docker compose down`.
///
/// When no targets are given, project-wide behaviour is unchanged:
///
/// 1. Resolve the project slug.
/// 2. Discover the orchestrator from any compose-tagged runner's labels;
///    `SIGTERM` it if alive (not in registry, so signals go via raw pid).
/// 3. `SIGTERM` every non-terminal runner via [`ProcessHandle::stop`].
/// 4. Poll until everything is terminal or `--timeout` elapses, escalating
///    to `SIGKILL` on timeout.
///
/// When targets are given, only the named services are stopped. The
/// orchestrator and sibling services are left running.
///
/// # Errors
///
/// Forwarded from [`ComposeRuntimeError`].
#[allow(clippy::too_many_lines)]
pub async fn run_compose_down(args: &ComposeDownArgs) -> Result<(), ComposeRuntimeError> {
    let slug = runtime_project_slug(args.file.as_deref(), args.project_name.as_deref())?;
    let all_members = list_project_members(&slug)?;
    let targets = resolve_down_targets(args)?;
    let targeted = targets.is_some();

    if let Some(ref target_names) = targets {
        validate_targets_against_members(target_names, &all_members)?;
    }

    if all_members.is_empty() {
        if !args.quiet {
            cli_eprintln!("project {slug:?}: no runners registered");
        }
        return Ok(());
    }

    let members: Vec<&ProjectMember> = match targets {
        Some(ref names) => all_members
            .iter()
            .filter(|m| names.contains(&m.service))
            .collect(),
        None => all_members.iter().collect(),
    };

    let orchestrator = if targeted {
        None
    } else {
        let orch = find_active_orchestrator(&slug)?;
        if let Some(active) = orch.as_ref() {
            let signalled = signal_identity(&active.identity, PosixSignal::Term)?;
            if signalled && !args.quiet {
                cli_eprintln!(
                    "project {slug:?}: SIGTERM orchestrator pid {pid}",
                    pid = active.identity.pid.as_raw()
                );
            }
        }
        orch
    };

    let registry = ProcessRegistry::open_default()?;
    let mut handles: Vec<(String, String, ProcessHandle)> = Vec::with_capacity(members.len());
    for member in &members {
        let id = member.record.id();
        let handle = ProcessHandle::open(registry.proc_root(), id).await?;
        let status = handle.refresh_status().await?;
        if status.is_terminal() {
            if targeted && !args.quiet {
                cli_eprintln!(
                    "project {slug:?}: service {service:?} already stopped",
                    service = member.service,
                );
            }
            continue;
        }
        match handle.stop().await {
            Ok(_) => {}
            Err(ProcessError::IllegalTransition {
                observed: Some(o), ..
            }) if o.is_terminal() => {
                continue;
            }
            Err(err) => return Err(err.into()),
        }
        handles.push((id.to_string(), member.service.clone(), handle));
        if !args.quiet {
            cli_eprintln!(
                "project {slug:?}: SIGTERM service {service:?} ({id})",
                service = member.service,
                id = trunc_id(&id.to_string(), false)
            );
        }
    }

    let timeout = Duration::from_secs(args.timeout);
    let deadline = Instant::now() + timeout;
    let mut still_alive: Vec<(String, String, ProcessHandle)> = handles;
    let orchestrator_identity = orchestrator.as_ref().map(|a| a.identity.clone());
    loop {
        if !still_alive.is_empty() {
            let mut next: Vec<(String, String, ProcessHandle)> =
                Vec::with_capacity(still_alive.len());
            for (id, service, handle) in still_alive {
                let status = handle.refresh_status().await?;
                if !status.is_terminal() || record_pid_alive(&handle) {
                    next.push((id, service, handle));
                }
            }
            still_alive = next;
        }
        let orch_alive = orchestrator_identity
            .as_ref()
            .is_some_and(|id| process_is_alive_with_start_time(id).unwrap_or(true));
        if still_alive.is_empty() && !orch_alive {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }

    escalate_to_sigkill(&slug, &still_alive, orchestrator_identity.as_ref(), args).await?;

    if !targeted {
        sweep_late_members(&slug, &all_members, args).await?;
    }
    Ok(())
}

fn record_pid_alive(handle: &ProcessHandle) -> bool {
    let PidFileState::Found(identity) = handle.record().pid_identity() else {
        return false;
    };
    process_is_alive_with_start_time(&identity).unwrap_or(true)
}

async fn sweep_late_members(
    slug: &str,
    initial: &[ProjectMember],
    args: &ComposeDownArgs,
) -> Result<(), ComposeRuntimeError> {
    use std::collections::HashSet;
    let known: HashSet<String> = initial.iter().map(|m| m.record.id().to_string()).collect();
    let latecomers: Vec<ProjectMember> = list_project_members(slug)?
        .into_iter()
        .filter(|m| !known.contains(&m.record.id().to_string()))
        .filter(|m| !m.status.is_terminal())
        .collect();
    if latecomers.is_empty() {
        return Ok(());
    }
    if !args.quiet {
        cli_eprintln!(
            "project {slug:?}: sweeping {n} late-spawned runner(s) the orchestrator dropped",
            n = latecomers.len()
        );
    }
    let registry = ProcessRegistry::open_default()?;
    let mut first_error: Option<ComposeRuntimeError> = None;
    let mut record_error = |err: ComposeRuntimeError| {
        if first_error.is_none() {
            first_error = Some(err);
        }
    };
    for member in &latecomers {
        let id = member.record.id();
        let handle = match ProcessHandle::open(registry.proc_root(), id).await {
            Ok(h) => h,
            Err(err) => {
                cli_eprintln!("project {slug:?}: open handle for late-spawned {id} failed: {err}");
                record_error(err.into());
                continue;
            }
        };
        match handle.kill().await {
            Ok(_) => {}
            Err(ProcessError::IllegalTransition {
                observed: Some(o), ..
            }) if o.is_terminal() => {
                if let Err(err) = handle.force_kill() {
                    cli_eprintln!(
                        "project {slug:?}: force_kill on late-spawned {id} failed: {err}"
                    );
                    record_error(err.into());
                }
            }
            Err(err) => {
                cli_eprintln!("project {slug:?}: kill on late-spawned {id} failed: {err}");
                record_error(err.into());
            }
        }
    }
    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum KillResult {
    Delivered,
    AlreadyGone,
}

impl KillResult {
    fn from_force_kill(delivered: bool) -> Self {
        if delivered {
            Self::Delivered
        } else {
            Self::AlreadyGone
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn escalate_to_sigkill(
    slug: &str,
    still_alive: &[(String, String, ProcessHandle)],
    orchestrator_identity: Option<&iter_core::process::ProcessIdentity>,
    args: &ComposeDownArgs,
) -> Result<(), ComposeRuntimeError> {
    let mut first_error: Option<ComposeRuntimeError> = None;
    let mut record_error = |err: ComposeRuntimeError| {
        if first_error.is_none() {
            first_error = Some(err);
        }
    };

    for (id, service, handle) in still_alive {
        let status = match handle.refresh_status().await {
            Ok(s) => s,
            Err(err) => {
                cli_eprintln!(
                    "project {slug:?}: refresh_status for {service:?} ({id}) failed: {err}; falling back to force_kill",
                    id = trunc_id(id, false)
                );
                match handle.force_kill() {
                    Ok(true) => {
                        if !args.quiet {
                            cli_eprintln!(
                                "project {slug:?}: SIGKILL service {service:?} ({id}) via fallback after {timeout_s}s",
                                timeout_s = args.timeout,
                                id = trunc_id(id, false)
                            );
                        }
                    }
                    Ok(false) => {
                        if !args.quiet {
                            cli_eprintln!(
                                "project {slug:?}: service {service:?} ({id}) already gone (refresh_status failed but pid file shows exited)",
                                id = trunc_id(id, false)
                            );
                        }
                    }
                    Err(fk_err) => {
                        cli_eprintln!(
                            "project {slug:?}: fallback force_kill for {service:?} ({id}) failed: {fk_err}",
                            id = trunc_id(id, false)
                        );
                        record_error(err.into());
                        record_error(fk_err.into());
                    }
                }
                continue;
            }
        };
        let kill_result: Result<KillResult, ComposeRuntimeError> = if status.is_terminal() {
            handle
                .force_kill()
                .map(KillResult::from_force_kill)
                .map_err(Into::into)
        } else {
            match handle.kill().await {
                Ok(_) => Ok(KillResult::Delivered),
                Err(ProcessError::IllegalTransition {
                    observed: Some(o), ..
                }) if o.is_terminal() => handle
                    .force_kill()
                    .map(KillResult::from_force_kill)
                    .map_err(Into::into),
                Err(err) => Err(err.into()),
            }
        };
        match kill_result {
            Err(err) => {
                cli_eprintln!(
                    "project {slug:?}: SIGKILL service {service:?} ({id}) failed: {err}",
                    id = trunc_id(id, false)
                );
                record_error(err);
            }
            Ok(KillResult::Delivered) => {
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: SIGKILL service {service:?} ({id}) after {timeout_s}s",
                        timeout_s = args.timeout,
                        id = trunc_id(id, false)
                    );
                }
            }
            Ok(KillResult::AlreadyGone) => {
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: service {service:?} ({id}) already exited before SIGKILL",
                        id = trunc_id(id, false)
                    );
                }
            }
        }
    }

    if let Some(identity) = orchestrator_identity {
        match signal_identity(identity, PosixSignal::Kill) {
            Ok(true) => {
                let pid = identity.pid.as_raw();
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: SIGKILL orchestrator pid {pid} after {timeout_s}s",
                        timeout_s = args.timeout,
                    );
                }
                let kill_deadline = Instant::now() + Duration::from_secs(2);
                while process_is_alive_with_start_time(identity).unwrap_or(false)
                    && Instant::now() < kill_deadline
                {
                    sleep(Duration::from_millis(50)).await;
                }
            }
            Ok(false) => {}
            Err(err) => {
                let pid = identity.pid.as_raw();
                cli_eprintln!("project {slug:?}: SIGKILL orchestrator pid {pid} failed: {err}");
                record_error(err.into());
            }
        }
    }

    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(())
    }
}
