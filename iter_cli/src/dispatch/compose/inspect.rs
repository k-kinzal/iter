use crate::process::{PidFileState, process_is_alive_with_start_time};
use crate::{
    ComposePlan, ProjectMember, build, list_all_members_by_project, list_project_members,
    load_compose, read_trigger_status, trigger_state_root,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{ComposeConfigArgs, ComposeLsArgs, ComposePsArgs, ComposeValidateArgs};
use crate::output::{
    OutputFormat, Table, ValidateCounts, ValidateFormat, ValidateOk, cli_println, print_json_array,
    print_json_compact, print_ndjson_record, relative_time, trunc_id,
};

use super::{ComposePlanError, ComposeRuntimeError, resolve_compose_path, runtime_project_slug};

/// Handle `iter compose validate`.
///
/// # Errors
///
/// * The file does not exist or cannot be parsed.
/// * `build` rejects the plan.
pub(crate) fn run_compose_validate(args: &ComposeValidateArgs) -> Result<(), ComposePlanError> {
    let path = resolve_compose_path(args.file.as_deref());
    let root = load_compose(&path)?;
    let plan = build(&root, &path)?;
    match args.format {
        ValidateFormat::Text => cli_println!(
            "OK ({} queue, {} service, {} trigger)",
            plan.queue_count(),
            plan.service_count(),
            plan.trigger_count()
        ),
        ValidateFormat::Json => {
            let envelope = ValidateOk {
                ok: true,
                summary: ValidateCounts {
                    queues: plan.queue_count(),
                    services: plan.service_count(),
                    triggers: plan.trigger_count(),
                },
            };
            print_json_compact(&envelope).map_err(ComposePlanError::JsonSerialize)?;
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Clone)]
struct ComposeRow {
    kind: &'static str,
    name: String,
    detail: String,
}

/// Handle `iter compose config`.
///
/// Lists the queues, services, and triggers declared in the file as a
/// single elastic table with columns `KIND  NAME  DETAIL`. Does not
/// connect to any backend or check whether instances are actually running.
/// Mirrors `docker compose config`.
///
/// # Errors
///
/// Same as [`run_compose_validate`].
pub(crate) fn run_compose_config(args: &ComposeConfigArgs) -> Result<(), ComposePlanError> {
    let path = resolve_compose_path(args.file.as_deref());
    let root = load_compose(&path)?;
    let plan = build(&root, &path)?;
    let rows = collect_rows(&plan);

    if args.listing.quiet {
        for row in &rows {
            cli_println!("{}/{}", row.kind, row.name);
        }
        return Ok(());
    }

    match args.listing.format {
        OutputFormat::Json => {
            print_json_array(&rows).map_err(ComposePlanError::JsonSerialize)?;
        }
        OutputFormat::Table => {
            let mut table = Table::new(&["KIND", "NAME", "DETAIL"]);
            for row in &rows {
                table.row([row.kind.to_owned(), row.name.clone(), row.detail.clone()]);
            }
            table.print();
        }
    }
    Ok(())
}

fn collect_rows(plan: &ComposePlan) -> Vec<ComposeRow> {
    let mut rows = Vec::with_capacity(
        plan.queue_count()
            + plan.service_count()
            + plan.trigger_count()
            + usize::from(plan.telemetry().is_some()),
    );
    if plan.telemetry().is_some() {
        rows.push(ComposeRow {
            kind: "telemetry",
            name: "default".to_owned(),
            detail: "opentelemetry traces/logs".to_owned(),
        });
    }
    for name in plan.queue_names() {
        let detail = source_detail(plan, name);
        rows.push(ComposeRow {
            kind: "queue",
            name: name.to_owned(),
            detail,
        });
    }
    for name in plan.service_names() {
        let detail = source_detail(plan, name);
        rows.push(ComposeRow {
            kind: "service",
            name: name.to_owned(),
            detail,
        });
    }
    for name in plan.trigger_names() {
        let detail = source_detail(plan, name);
        rows.push(ComposeRow {
            kind: "trigger",
            name: name.to_owned(),
            detail,
        });
    }
    rows
}

fn source_detail(plan: &ComposePlan, name: &str) -> String {
    match plan.source_of(name) {
        Some(path) => format!("from {}", path.display()),
        None => String::new(),
    }
}

#[derive(Debug, Serialize)]
struct ComposeLsRow {
    name: String,
    services: usize,
    runners: usize,
    status: String,
    orchestrator_pid: Option<u32>,
}

/// Handle `iter compose ls`. Mirrors `docker compose ls`.
///
/// Walks the local registry, groups every runner carrying
/// `iter.compose.project = <slug>` by project, and reports the
/// orchestrator-liveness status.
///
/// # Errors
///
/// Returns [`ComposeRuntimeError::Discovery`] if scanning the registry or
/// reading runner labels fails.
pub(crate) fn run_compose_ls(args: &ComposeLsArgs) -> Result<(), ComposeRuntimeError> {
    let by_project = list_all_members_by_project()?;
    let mut rows: Vec<ComposeLsRow> = Vec::with_capacity(by_project.len());
    for (project, members) in by_project {
        if project.is_empty() {
            continue;
        }
        let row = build_ls_row(project, &members);
        if !args.all && row.orchestrator_pid.is_none() {
            continue;
        }
        rows.push(row);
    }

    if args.listing.quiet {
        for row in &rows {
            cli_println!("{}", row.name);
        }
        return Ok(());
    }

    match args.listing.format {
        OutputFormat::Json => {
            for row in &rows {
                print_ndjson_record(row).map_err(ComposeRuntimeError::JsonSerialize)?;
            }
        }
        OutputFormat::Table => {
            let mut table = Table::new(&["NAME", "SERVICES", "RUNNERS", "STATUS", "ORCH PID"]);
            for row in &rows {
                table.row([
                    row.name.clone(),
                    row.services.to_string(),
                    row.runners.to_string(),
                    row.status.clone(),
                    row.orchestrator_pid
                        .map_or_else(|| "-".to_owned(), |p| p.to_string()),
                ]);
            }
            table.print();
        }
    }
    Ok(())
}

fn build_ls_row(project: String, members: &[ProjectMember]) -> ComposeLsRow {
    use std::collections::BTreeSet;
    let services: BTreeSet<&str> = members
        .iter()
        .filter(|m| !m.service.is_empty())
        .map(|m| m.service.as_str())
        .collect();
    let live_count = members.iter().filter(|m| !m.status.is_terminal()).count();
    let orchestrator_pid = members.iter().find_map(|m| {
        process_is_alive_with_start_time(&m.orchestrator)
            .ok()
            .and_then(|alive| alive.then_some(m.orchestrator.pid.as_raw()))
    });
    let status = if orchestrator_pid.is_some() {
        format!("running({live_count})")
    } else if live_count == 0 {
        "exited".to_owned()
    } else {
        format!("orphaned({live_count})")
    };
    ComposeLsRow {
        name: project,
        services: services.len(),
        runners: members.len(),
        status,
        orchestrator_pid,
    }
}

#[derive(Debug, Serialize)]
struct ComposePsRow {
    id: String,
    service: String,
    status: String,
    pid: Option<u32>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct ComposePsTriggerRow {
    trigger: String,
    kind: String,
    status: String,
    restart_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    last_state_change: DateTime<Utc>,
}

/// Handle `iter compose ps`. Mirrors `docker compose ps` for a single
/// project.
///
/// # Errors
///
/// Returns [`ComposeRuntimeError::ProjectSlug`] if the slug cannot be
/// derived, or [`ComposeRuntimeError::Discovery`] if the registry scan
/// fails.
pub(crate) fn run_compose_ps(args: &ComposePsArgs) -> Result<(), ComposeRuntimeError> {
    let slug = runtime_project_slug(args.file.as_deref(), args.project_name.as_deref())?;
    let members = list_project_members(&slug)?;
    let mut rows: Vec<ComposePsRow> = Vec::with_capacity(members.len());
    for member in &members {
        if !args.all && member.status.is_terminal() {
            continue;
        }
        let pid = match member.record.pid_identity() {
            PidFileState::Found(identity) => Some(identity.pid.as_raw()),
            _ => None,
        };
        rows.push(ComposePsRow {
            id: member.record.id().to_string(),
            service: member.service.clone(),
            status: member.status.as_serde_str().to_owned(),
            pid,
            created_at: member.started_at,
        });
    }

    let trigger_rows = collect_trigger_status_rows(&slug);

    if args.listing.quiet {
        for row in &rows {
            cli_println!("{}", trunc_id(&row.id, args.listing.no_trunc));
        }
        return Ok(());
    }

    match args.listing.format {
        OutputFormat::Json => {
            for row in &rows {
                print_ndjson_record(row).map_err(ComposeRuntimeError::JsonSerialize)?;
            }
            for trow in &trigger_rows {
                print_ndjson_record(trow).map_err(ComposeRuntimeError::JsonSerialize)?;
            }
        }
        OutputFormat::Table => {
            let mut table = Table::new(&["ID", "SERVICE", "STATUS", "PID", "CREATED"]);
            for row in &rows {
                table.row([
                    trunc_id(&row.id, args.listing.no_trunc),
                    row.service.clone(),
                    row.status.clone(),
                    row.pid.map_or_else(|| "?".to_owned(), |p| p.to_string()),
                    relative_time(row.created_at),
                ]);
            }
            if !trigger_rows.is_empty() {
                table.row(["---", "TRIGGERS", "---", "---", "---"]);
                for trow in &trigger_rows {
                    let restarts = if trow.restart_count > 0 {
                        format!("({}x)", trow.restart_count)
                    } else {
                        String::new()
                    };
                    table.row([
                        format!("[{}]", trow.kind),
                        trow.trigger.clone(),
                        format!("{} {restarts}", trow.status),
                        "-".into(),
                        relative_time(trow.last_state_change),
                    ]);
                }
            }
            table.print();
        }
    }
    Ok(())
}

fn collect_trigger_status_rows(project: &str) -> Vec<ComposePsTriggerRow> {
    let Some(root) = trigger_state_root() else {
        return Vec::new();
    };
    let project_dir = root.join(project);
    let Ok(entries) = std::fs::read_dir(&project_dir) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let trigger_dir = entry.path();
        if !trigger_dir.is_dir() {
            continue;
        }
        if let Some(status) = read_trigger_status(&trigger_dir) {
            rows.push(ComposePsTriggerRow {
                trigger: status.name,
                kind: status.kind,
                status: status.state.to_string(),
                restart_count: status.restart_count,
                last_error: status.last_error,
                last_state_change: status.last_state_change,
            });
        }
    }
    rows.sort_by(|a, b| a.trigger.cmp(&b.trigger));
    rows
}
