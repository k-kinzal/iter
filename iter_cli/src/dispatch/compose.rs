//! `iter compose` — `up`, `validate`, `config`, `ls`, `ps`, `down` dispatchers.
//!
//! Each public function here corresponds to one variant of
//! [`crate::cli::ComposeCmd`]. The heavy lifting lives in
//! [`iter_compose`] — these handlers are thin wrappers that resolve the
//! file path, install the shutdown handler, render diagnostics, and forward
//! the result.
//!
//! `config` is the static plan listing (queues / services / triggers parsed
//! from `compose.iter`). `ls` / `ps` / `down` operate on the **runtime**
//! state reconstructed from `iter.compose.*` labels stamped onto child
//! runners — mirroring how `docker compose ps` reads container labels.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use iter_compose::iterfile::RunRecordMetadata;
use iter_compose::signals::SignalsError;
use iter_compose::{
    ComposeError, ComposePlan, DEFAULT_COMPOSE_FILE, DiscoveryError, FailurePolicy,
    OrchestratorContext, ProjectLockError, ProjectMember, ProjectSlugError, acquire_project_lock,
    build, find_active_orchestrator, install_shutdown_handler, is_compose_filename,
    list_all_members_by_project, list_project_members, load_compose, project_slug,
    read_trigger_status, run, trigger_state_root,
};
use iter_core::process::{
    PidFileState, ProcessError, ProcessHandle, ProcessRegistry, SignalDelivery, UnmanagedChild,
    current_identity, pid_in_process_table, process_is_alive_with_start_time, signal_identity,
    signal_pid_kill, signal_pid_term, spawn_unmanaged_detached,
};
use serde::Serialize;
use thiserror::Error;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::cli::{
    ComposeConfigArgs, ComposeDownArgs, ComposeFailure, ComposeLsArgs, ComposePsArgs,
    ComposeUpArgs, ComposeValidateArgs,
};
use crate::dispatch::load::{LoadError, load_iterfile};
use crate::output::{
    IntoExitCode, OutputFormat, Table, ValidateFormat, cli_eprintln, cli_println, exit_codes,
    print_json_array, print_json_compact, print_ndjson_record, relative_time, trunc_id,
};
use crate::telemetry;

/// Errors produced by `iter compose up`.
///
/// Heavy underlying error types (`ProcessError`, `ComposeError`) are boxed
/// so the enum stays inside clippy's `result_large_err` budget.
#[derive(Debug, Error)]
pub enum ComposeUpError {
    /// Loading or building the compose file failed.
    #[error(transparent)]
    Compose(Box<ComposeError>),
    /// Installing the shutdown signal handler failed.
    #[error(transparent)]
    Signals(#[from] SignalsError),
    /// At least one spawned task ended in an error.
    #[error("one or more compose tasks failed")]
    TaskFailed,
    /// Resolving the compose file path or canonicalising it failed.
    #[error("locating compose file at {}: {source}", path.display())]
    ComposeFileMissing {
        /// The path the user named.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Locating the current `iter` executable for the detach self-fork failed.
    #[error("locating current executable: {0}")]
    CurrentExe(#[source] std::io::Error),
    /// Forking the detached orchestrator child failed.
    #[error("spawning detached compose orchestrator: {0}")]
    Spawn(#[source] std::io::Error),
    /// Deriving the docker-compose-style project slug failed.
    #[error(transparent)]
    ProjectSlug(Box<ProjectSlugError>),
    /// Capturing the orchestrator's own [`ProcessIdentity`](iter_core::process::ProcessIdentity)
    /// at startup failed; without it we cannot stamp orchestrator-discovery
    /// labels onto child runners.
    #[error("collecting orchestrator identity: {0}")]
    OrchestratorIdentity(#[source] Box<ProcessError>),
    /// Scanning the registry for an existing project orchestrator failed.
    #[error("scanning registry for active project: {0}")]
    Discovery(#[source] Box<DiscoveryError>),
    /// A live orchestrator already owns this project slug.
    #[error(
        "project {project:?} is already up (orchestrator pid {orchestrator_pid}); \
         use `iter compose down` to stop it first, or pass -p/--project-name \
         to use a different project"
    )]
    ProjectAlreadyUp {
        /// The colliding project slug.
        project: String,
        /// pid of the live orchestrator (for human diagnosis only —
        /// discovery uses pid + start-time fingerprint).
        orchestrator_pid: u32,
    },
    /// `compose up -d` spawned an orchestrator that did not register any
    /// service runner before the readiness deadline. The orchestrator has
    /// already been signalled before this error surfaces, so the caller
    /// can safely return.
    #[error(
        "compose orchestrator did not start any service for project {project:?} \
         within {timeout_secs}s; orchestrator pid {orchestrator_pid} was signalled"
    )]
    OrchestratorStartupTimeout {
        /// The project slug whose orchestrator failed to make progress.
        project: String,
        /// pid of the orchestrator that we just signalled.
        orchestrator_pid: u32,
        /// How long we waited before giving up.
        timeout_secs: u64,
    },
    /// `compose up -d` spawned an orchestrator that exited before
    /// registering a service. Most often this means the orchestrator hit
    /// a runtime error (e.g. queue init / secret resolution) that
    /// pre-validation cannot catch. stderr was redirected to `/dev/null`
    /// so we have no detail to forward.
    #[error(
        "compose orchestrator for project {project:?} exited before registering \
         any service (orchestrator pid {orchestrator_pid}); rerun without -d to see the failure"
    )]
    OrchestratorExitedEarly {
        /// The project slug whose orchestrator died.
        project: String,
        /// pid that exited.
        orchestrator_pid: u32,
    },
    /// Targeted `compose up SERVICE` requires `--detach`.
    #[error("targeted compose up requires --detach; foreground mode is project-wide only")]
    TargetedRequiresDetach,
    /// One or more service targets are unknown.
    #[error(
        "unknown service target(s): {unknown}; valid services: {valid}",
        unknown = unknown.join(", "),
        valid = if valid.is_empty() { "(none)".to_owned() } else { valid.join(", ") }
    )]
    UnknownTargets {
        /// Targets that did not match any service.
        unknown: Vec<String>,
        /// Valid service names for the diagnostic.
        valid: Vec<String>,
    },
    /// A target string uses an unsupported resource type prefix.
    #[error(
        "unsupported resource type in target `{target}`; only bare names \
         and `service/NAME` are supported"
    )]
    UnsupportedResourceType {
        /// The full target string.
        target: String,
    },
    /// A targeted service's queue is not URL-addressable.
    #[error(transparent)]
    TargetedSpawn(#[from] iter_compose::TargetedSpawnError),
    /// `--source` did not match any service's build path.
    #[error(
        "--source {} does not match any service's build path in the compose file",
        path.display()
    )]
    SourceNoMatch {
        /// The path the user named.
        path: PathBuf,
    },
    /// Acquiring or releasing the per-project advisory lock failed.
    /// `AlreadyHeld` collapses to a clearer "another invocation is
    /// starting this project" message; the variant carries the source
    /// for everything else (e.g. `mkdir` failures).
    #[error(transparent)]
    ProjectLock(#[from] ProjectLockError),
}

impl From<ComposeError> for ComposeUpError {
    fn from(value: ComposeError) -> Self {
        Self::Compose(Box::new(value))
    }
}

impl From<ProjectSlugError> for ComposeUpError {
    fn from(value: ProjectSlugError) -> Self {
        Self::ProjectSlug(Box::new(value))
    }
}

impl From<DiscoveryError> for ComposeUpError {
    fn from(value: DiscoveryError) -> Self {
        Self::Discovery(Box::new(value))
    }
}

impl IntoExitCode for ComposeUpError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Compose(e) => compose_error_exit_code(e),
            // `AlreadyHeld` was rewritten to `ProjectAlreadyUp`
            // upstream so any `ProjectLock` reaching here is a real
            // local I/O failure (mkdir / open) — runtime, not user
            // input.
            Self::Signals(_)
            | Self::TaskFailed
            | Self::Spawn(_)
            | Self::OrchestratorStartupTimeout { .. }
            | Self::OrchestratorExitedEarly { .. }
            | Self::ProjectLock(_) => exit_codes::RUNTIME,
            Self::ComposeFileMissing { .. }
            | Self::ProjectSlug(_)
            | Self::ProjectAlreadyUp { .. }
            | Self::TargetedRequiresDetach
            | Self::UnknownTargets { .. }
            | Self::UnsupportedResourceType { .. }
            | Self::TargetedSpawn(_)
            | Self::SourceNoMatch { .. } => exit_codes::USER_INPUT,
            Self::CurrentExe(_) | Self::OrchestratorIdentity(_) | Self::Discovery(_) => {
                exit_codes::INTERNAL
            }
        }
    }
}

/// Errors produced by `iter compose validate` / `iter compose config`.
#[derive(Debug, Error)]
pub enum ComposePlanError {
    /// Loading or building the compose file failed.
    #[error(transparent)]
    Compose(#[from] ComposeError),
    /// Serialising the listing to JSON failed.
    #[error("serializing compose listing: {0}")]
    JsonSerialize(#[source] serde_json::Error),
}

impl IntoExitCode for ComposePlanError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Compose(e) => compose_error_exit_code(e),
            Self::JsonSerialize(_) => exit_codes::INTERNAL,
        }
    }
}

/// Errors produced by the runtime-listing handlers
/// (`iter compose ls` / `iter compose ps` / `iter compose down`).
///
/// These commands work against the runtime registry rather than a
/// `compose.iter` plan, so their failures live in their own enum.
#[derive(Debug, Error)]
pub enum ComposeRuntimeError {
    /// Walking the local registry to discover compose-tagged runners failed.
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    /// Resolving the project slug for `ps` / `down` failed.
    #[error(transparent)]
    ProjectSlug(Box<ProjectSlugError>),
    /// Opening a runner handle to deliver `SIGTERM`/`SIGKILL` failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// Serialising the listing to JSON failed.
    #[error("serializing compose listing: {0}")]
    JsonSerialize(#[source] serde_json::Error),
    /// One or more service targets have no registered runner in the
    /// project. The service may exist in the compose plan but has no
    /// live or terminal process record — use `iter compose ps --all`
    /// to inspect the registry.
    #[error(
        "no runners registered for service target(s): {unknown}; \
         currently running services: {valid}",
        unknown = unknown.join(", "),
        valid = if valid.is_empty() { "(none)".to_owned() } else { valid.join(", ") }
    )]
    UnknownTargets {
        /// Targets that did not match any registered runner.
        unknown: Vec<String>,
        /// Non-terminal service names for the diagnostic.
        valid: Vec<String>,
    },
    /// A target string uses an unsupported resource type prefix.
    #[error(
        "unsupported resource type in target `{target}`; only bare names \
         and `service/NAME` are supported"
    )]
    UnsupportedResourceType {
        /// The full target string.
        target: String,
    },
    /// Loading or building the compose file failed (needed by `--source`).
    #[error(transparent)]
    Compose(Box<ComposeError>),
    /// `--source` did not match any service's build path.
    #[error(
        "--source {} does not match any service's build path in the compose file",
        path.display()
    )]
    SourceNoMatch {
        /// The path the user named.
        path: PathBuf,
    },
}

impl From<ProjectSlugError> for ComposeRuntimeError {
    fn from(value: ProjectSlugError) -> Self {
        Self::ProjectSlug(Box::new(value))
    }
}

impl From<ComposeError> for ComposeRuntimeError {
    fn from(value: ComposeError) -> Self {
        Self::Compose(Box::new(value))
    }
}

impl IntoExitCode for ComposeRuntimeError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::ProjectSlug(_)
            | Self::UnknownTargets { .. }
            | Self::UnsupportedResourceType { .. }
            | Self::SourceNoMatch { .. } => exit_codes::USER_INPUT,
            Self::Discovery(_) | Self::Process(_) => exit_codes::RUNTIME,
            Self::Compose(e) => compose_error_exit_code(e),
            Self::JsonSerialize(_) => exit_codes::INTERNAL,
        }
    }
}

/// Map a [`ComposeError`] to a typed exit code.
///
/// Shared by `iter compose validate`/`ls` (via [`ComposePlanError`]) and
/// `iter enqueue` (via `EnqueueCmdError::Compose`). A single source of
/// truth is the only way to keep `iter compose ls -f missing.iter` and
/// `iter enqueue -f missing.iter` agreeing on `USER_INPUT (1)` rather
/// than drifting apart silently.
pub(crate) fn compose_error_exit_code(e: &ComposeError) -> i32 {
    match e {
        ComposeError::Io { .. }
        | ComposeError::UnknownQueue(_)
        | ComposeError::UnknownService(_) => exit_codes::USER_INPUT,
        ComposeError::Parse { .. }
        | ComposeError::BuildTargetHasQueue { .. }
        | ComposeError::ServiceMissingSection { .. }
        | ComposeError::ArgResolve { .. }
        | ComposeError::AgentBuild { .. }
        | ComposeError::PromptBuild { .. }
        | ComposeError::EventTemplate { .. }
        | ComposeError::Builder { .. }
        | ComposeError::NoServices { .. }
        | ComposeError::CircularComposeImport { .. }
        | ComposeError::ComposeNameCollision { .. }
        | ComposeError::UnknownChildQueue { .. }
        | ComposeError::UnknownChildService { .. }
        | ComposeError::UnknownChildTrigger { .. }
        | ComposeError::UnsupportedTriggerKind { .. } => exit_codes::CONFIG,
        ComposeError::Queue(_) | ComposeError::QueueBuild { .. } => exit_codes::RUNTIME,
        ComposeError::UnresolvedAnonymousQueueRef | ComposeError::UnresolvedServiceQueue(_) => {
            exit_codes::INTERNAL
        }
    }
}

/// Errors produced by [`validate_path_autodetect`].
#[derive(Debug, Error)]
pub enum ValidateAutodetectError {
    /// Validating an Iterfile failed.
    #[error(transparent)]
    Load(#[from] LoadError),
    /// Validating a `compose.iter` failed.
    #[error("validating compose file at {}: {source}", path.display())]
    Compose {
        /// Resolved compose file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: Box<ComposePlanError>,
    },
    /// Serialising the validate-JSON envelope failed.
    #[error("serializing validate output: {0}")]
    JsonSerialize(#[source] serde_json::Error),
}

impl IntoExitCode for ValidateAutodetectError {
    fn exit_code(&self) -> i32 {
        match self {
            // Delegate to LoadError so a missing iterfile (USER_INPUT)
            // does not get re-classified as a parse-level CONFIG error.
            Self::Load(e) => e.exit_code(),
            Self::Compose { source, .. } => source.exit_code(),
            Self::JsonSerialize(_) => exit_codes::INTERNAL,
        }
    }
}

/// Aggregate compose-subcommand error.
#[derive(Debug, Error)]
pub enum ComposeCmdError {
    /// `iter compose up` failure.
    #[error(transparent)]
    Up(#[from] ComposeUpError),
    /// `iter compose validate` / `iter compose config` failure.
    #[error(transparent)]
    Plan(#[from] ComposePlanError),
    /// `iter compose ls` / `iter compose ps` / `iter compose down` failure.
    #[error(transparent)]
    Runtime(#[from] ComposeRuntimeError),
}

impl IntoExitCode for ComposeCmdError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Up(e) => e.exit_code(),
            Self::Plan(e) => e.exit_code(),
            Self::Runtime(e) => e.exit_code(),
        }
    }
}

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
pub async fn run_compose_up(args: ComposeUpArgs) -> Result<(), ComposeUpError> {
    let has_targets = !args.targets.is_empty() || args.source.is_some();
    if has_targets {
        return run_compose_up_targeted(args).await;
    }
    if args.detach {
        return spawn_compose_detached(&args);
    }
    run_compose_up_inline(args).await
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

/// [`parse_target_name`] mirror for [`ComposeUpError`].
fn parse_target_name_for_up(target: &str) -> Result<&str, ComposeUpError> {
    parse_target_name_raw(target).map_err(|target| ComposeUpError::UnsupportedResourceType {
        target: target.to_owned(),
    })
}

/// In-process orchestrator path. Used directly by foreground `compose up`,
/// and entered by the post-fork child of `--detach` (with stdio already
/// redirected to `/dev/null`).
async fn run_compose_up_inline(args: ComposeUpArgs) -> Result<(), ComposeUpError> {
    let raw_path = resolve_compose_path(args.file.as_deref());
    // Canonicalize for the same reason `spawn_compose_detached` does:
    // both up paths must derive the same project slug from the same
    // `-f`, regardless of whether the user supplied a symlink or a
    // relative path (Codex iter-2 Minor 1).
    let path = canonical_compose_path(&raw_path)?;
    let root = load_compose(&path)?;
    let plan = build(&root, &path)?;
    let config = iter_core::Config::default();
    let project = project_slug(&path, args.project_name.as_deref())?;
    let _telemetry_guard =
        telemetry::init_for_compose(args.debug, &config, plan.telemetry(), &project, None);
    info!(
        compose = %path.display(),
        queues = plan.queue_count(),
        services = plan.service_count(),
        "starting compose"
    );

    // Capture the orchestrator's identity once before any service spawns,
    // so every child runner gets stamped with the same pid + start_time
    // fingerprint (labels-based discovery for `compose ls/ps/down`).
    let identity =
        current_identity().map_err(|e| ComposeUpError::OrchestratorIdentity(Box::new(e)))?;
    let orchestrator = OrchestratorContext { project, identity };

    let cancel = install_shutdown_handler(CancellationToken::new())?;
    let policy = match args.on_failure {
        ComposeFailure::Abort => FailurePolicy::AbortAll,
        ComposeFailure::Continue => FailurePolicy::Continue,
    };
    let metadata = RunRecordMetadata {
        argv: rebuild_argv(&args),
        subcommand: "compose up".into(),
        debug: args.debug,
    };
    let report = run(plan, cancel, policy, metadata, None, orchestrator).await;

    if report.has_errors() {
        for outcome in &report.outcomes {
            if outcome.is_err() {
                error!(task = outcome.name(), "compose task exited with error");
            }
        }
        return Err(ComposeUpError::TaskFailed);
    }

    info!(
        completed = report.outcomes.len(),
        "compose finished cleanly"
    );
    Ok(())
}

/// Fork the orchestrator into the background with `setsid` and stdio
/// redirected to `/dev/null`, then wait synchronously until the
/// orchestrator either registers its first service runner or fails.
///
/// Pre-flight steps run in the parent (so failures surface immediately
/// instead of being swallowed by the child's `/dev/null` stderr):
///
/// 1. Resolve and canonicalise the compose path.
/// 2. `load_compose` + `build` — catches HCL parse errors and plan-build
///    rejections (Codex Finding 1).
/// 3. Compute the project slug.
/// 4. Refuse a duplicate `up -d` if an existing orchestrator is alive.
///
/// After the fork the parent polls `~/.iter/proc/` for runners labelled
/// with our slug. Once at least one appears (any state), `compose down`
/// can discover the orchestrator via labels — closing the race the plan
/// previously accepted (Codex Finding 3). On timeout or early
/// orchestrator exit the parent signals the orchestrator and returns an
/// error so the user is never left with a silent zombie.
///
/// The child argv carries `--project-name <slug>` for human
/// discoverability (e.g. `pgrep -f "iter compose up.*--project-name demo"`);
/// discovery in the `compose ls/ps/down` codepaths uses the labels
/// stamped on child runners rather than scraping argv.
fn spawn_compose_detached(args: &ComposeUpArgs) -> Result<(), ComposeUpError> {
    let program = std::env::current_exe().map_err(ComposeUpError::CurrentExe)?;
    let raw_path = resolve_compose_path(args.file.as_deref());
    let compose_path = canonical_compose_path(&raw_path)?;

    // Pre-validate: load + build in the parent so HCL parse errors and
    // plan-build rejections fail synchronously with a real exit code,
    // rather than being swallowed by the child's `/dev/null` stderr.
    // Order (load → build → slug) intentionally matches the foreground
    // path so both surface the same first-failure for a given input
    // (Codex iter-2 Minor 1).
    let root = load_compose(&compose_path)?;
    let _plan = build(&root, &compose_path)?;
    let slug = project_slug(&compose_path, args.project_name.as_deref())?;

    // Hold a per-project advisory `flock` across both the liveness
    // check and the double-fork. Without this, two `compose up -d`
    // invocations for the same slug can both observe "no orchestrator
    // running" and both fork a fresh orchestrator (Codex iter-9
    // Major 1). `AlreadyHeld` here means another `compose up` is
    // already inside this critical section — surface it as the same
    // "project already up" error from the user's perspective so the
    // race and the steady-state collision present identically.
    let _lock = match acquire_project_lock(&slug) {
        Ok(guard) => guard,
        Err(ProjectLockError::AlreadyHeld { project }) => {
            // Best-effort enrichment: report the live orchestrator's
            // pid if we can find it; otherwise fall back to the bare
            // "already starting" message.
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

    // Refuse to fork a second orchestrator for a project whose previous
    // orchestrator is still alive. Mirrors `docker compose up -d`'s
    // "service is already running" behaviour.
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
    // Always emit --project-name so child receives the resolved slug
    // (also makes the orchestrator visible via `pgrep -f --project-name`).
    child_args.push("--project-name".into());
    child_args.push(slug.clone());

    let child =
        spawn_unmanaged_detached(&program, &child_args, &[]).map_err(ComposeUpError::Spawn)?;

    wait_for_orchestrator_ready(&slug, child)
}

/// Block until the just-spawned orchestrator has registered at least one
/// service runner whose `iter.compose.orchestrator_pid` label matches
/// the pid we control. Matching on the pid (not just the project slug)
/// is critical: stale terminal records left by a previous run carry
/// the same `iter.compose.project` label, so a slug-only check would
/// return success against the *previous* orchestrator's debris before
/// the new one had registered anything (Codex iter-2 Major 1).
///
/// While polling we also `try_wait` the [`UnmanagedChild`] so an early
/// orchestrator exit is detected immediately rather than waiting out the
/// 30s timeout (Codex iter-2 Major 2). Any discovery error in the loop
/// triggers the same SIGTERM/SIGKILL cleanup as a timeout, so we never
/// leak a silent zombie on the error path either (Codex iter-2 Major 4).
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
                    // Detach the Child so its Drop does not block on wait.
                    child.detach();
                    return Ok(());
                }
            }
            Err(err) => {
                // Discovery itself broke. We can't tell whether the
                // orchestrator is healthy, so we err on the side of
                // tearing it down — the alternative is a silent zombie
                // with stderr at /dev/null.
                kill_orphan(orchestrator_pid);
                child.detach();
                return Err(err.into());
            }
        }

        // Detect early child exit without waiting for the timeout.
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
                // try_wait failed for some reason; fall back to the bare
                // kill(pid, 0) check below.
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

/// Best-effort SIGTERM → SIGKILL escalation for the orchestrator pid
/// when we have decided to abort the readiness wait. We hold the Child
/// handle, so the process is reaped via wait when the caller drops it.
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

/// True iff `member` is a non-terminal record whose orchestrator-pid
/// label matches `expected_pid`. Used by [`wait_for_orchestrator_ready`]
/// to ignore stale terminal records left by a previous `compose up`
/// session under the same slug.
///
/// # Why a raw-pid check is safe here
///
/// We could compare the full [`ProcessIdentity`] for reuse-resistance,
/// but `wait_for_orchestrator_ready` is the **parent** of the
/// orchestrator and it is still holding the [`UnmanagedChild`] handle
/// while it polls. The kernel cannot reap and recycle a child's pid
/// until the parent calls `wait(2)` or detaches the handle — neither
/// happens until *after* this function returns success. A raw-pid match
/// therefore cannot collide with a different process. Outside this
/// critical section (e.g. `compose down`, `compose ls`) callers must
/// use [`signal_identity`] or [`process_is_alive_with_start_time`] for
/// proper TOCTOU narrowing.
///
/// The orchestrator pid is read from [`ProjectMember::orchestrator`],
/// which was populated from the same `meta.json` read that built the
/// rest of the member — no second read, no `ENOENT` race against a
/// concurrent `iter rm`.
fn matches_spawned_orchestrator(member: &ProjectMember, expected_pid: u32) -> bool {
    if member.status.is_terminal() {
        return false;
    }
    member.orchestrator.pid.as_raw() == expected_pid
}

/// Upper bound on how long the parent of `compose up -d` waits for its
/// orchestrator to make observable progress (i.e. register the first
/// service runner) before treating the spawn as a failure.
const ORCHESTRATOR_READY_TIMEOUT_SECS: u64 = 30;

fn canonical_compose_path(raw: &Path) -> Result<PathBuf, ComposeUpError> {
    canonicalize_compose_path(raw).map_err(|source| ComposeUpError::ComposeFileMissing {
        path: raw.to_path_buf(),
        source,
    })
}

/// Resolve and canonicalize a `-f` argument the same way across
/// `compose up`, `compose ps`, and `compose down`. Returning the same
/// path from all three guarantees that [`project_slug`] derives the
/// same slug regardless of which subcommand the user typed first
/// (Codex iter-2 Minor 1 — symlinks / relative paths used to drift).
fn canonicalize_compose_path(raw: &Path) -> std::io::Result<PathBuf> {
    raw.canonicalize()
}

/// Slug helper shared by `compose ps` and `compose down`. Canonicalises
/// the `-f` argument first so a file-level symlink (e.g.
/// `-f /tmp/link.iter -> /projects/myapp/compose.iter`) yields the same
/// slug here as `compose up` did when the project was started.
///
/// Without this, `up` derives the slug from the resolved parent
/// (`myapp`) but `ps`/`down` would derive it from the symlink's parent
/// (`tmp`), and the two commands would silently disagree on which
/// project they were operating on (Codex iter-9 Major 5).
///
/// If `Path::canonicalize` fails (the file does not exist), we fall
/// back to the raw path so [`project_slug`] can surface its own typed
/// error — and so a `-p`-only invocation against a non-existent file
/// still works (the override wins before the file is consulted).
fn runtime_project_slug(
    file: Option<&Path>,
    override_name: Option<&str>,
) -> Result<String, ComposeRuntimeError> {
    let raw = resolve_compose_path(file);
    let path = canonicalize_compose_path(&raw).unwrap_or(raw);
    Ok(project_slug(&path, override_name)?)
}

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

/// Validate-summary envelope shared by `iter validate --format json` and
/// `iter compose validate --format json`.
#[derive(Debug, Serialize)]
struct ValidateOk {
    ok: bool,
    summary: ValidateSummary,
}

#[derive(Debug, Serialize)]
struct ValidateSummary {
    queues: usize,
    services: usize,
    triggers: usize,
}

/// Handle `iter compose validate`.
///
/// # Errors
///
/// * The file does not exist or cannot be parsed.
/// * `build` rejects the plan.
pub fn run_compose_validate(args: &ComposeValidateArgs) -> Result<(), ComposePlanError> {
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
                summary: ValidateSummary {
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

/// One row in the `iter compose config` table / JSON output.
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
pub fn run_compose_config(args: &ComposeConfigArgs) -> Result<(), ComposePlanError> {
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

fn resolve_compose_path(path: Option<&Path>) -> PathBuf {
    match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(DEFAULT_COMPOSE_FILE),
    }
}

/// One row in the `iter compose ls` table / NDJSON output.
///
/// Reconstructed entirely from `iter.compose.*` labels stamped onto child
/// runners — there is no orchestrator-side state file to consult.
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
/// orchestrator-liveness status reconstructed from the
/// `iter.compose.orchestrator_pid` + `iter.compose.orchestrator_start_time`
/// labels via [`process_is_alive_with_start_time`].
///
/// # Errors
///
/// Returns [`ComposeRuntimeError::Discovery`] if scanning the registry or
/// reading runner labels fails.
pub fn run_compose_ls(args: &ComposeLsArgs) -> Result<(), ComposeRuntimeError> {
    let by_project = list_all_members_by_project()?;
    let mut rows: Vec<ComposeLsRow> = Vec::with_capacity(by_project.len());
    for (project, members) in by_project {
        if project.is_empty() {
            continue;
        }
        let row = build_ls_row(project, &members);
        // Default to docker-compose-ls semantics: only projects whose
        // orchestrator (or at least one runner) is still alive. `--all`
        // restores the previous behaviour where terminal projects also
        // appear so the user can inspect their history.
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

/// One row in the `iter compose ps` table / NDJSON output.
#[derive(Debug, Serialize)]
struct ComposePsRow {
    id: String,
    service: String,
    status: String,
    pid: Option<u32>,
    created_at: DateTime<Utc>,
}

/// One row for a trigger in the `iter compose ps` output.
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
/// project. The project slug is resolved from `-p` or the compose file's
/// directory basename.
///
/// # Errors
///
/// Returns [`ComposeRuntimeError::ProjectSlug`] if the slug cannot be
/// derived, or [`ComposeRuntimeError::Discovery`] if the registry scan
/// fails.
pub fn run_compose_ps(args: &ComposePsArgs) -> Result<(), ComposeRuntimeError> {
    let slug = runtime_project_slug(args.file.as_deref(), args.project_name.as_deref())?;
    let members = list_project_members(&slug)?;
    let mut rows: Vec<ComposePsRow> = Vec::with_capacity(members.len());
    for member in &members {
        // Default to docker-compose-ps semantics: terminal runners are
        // hidden. `--all` shows them so users can inspect history.
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

/// Extract the service name from a target string, returning `Err(target)`
/// when the target uses an unsupported resource type prefix.
///
/// Accepts bare names (`worker-a`) and explicit `service/NAME` references.
/// Rejects empty names, names containing `/` after the prefix (e.g.
/// `service/service/foo`), and unknown resource type prefixes.
fn parse_target_name_raw(target: &str) -> Result<&str, &str> {
    if let Some(name) = target.strip_prefix("service/") {
        if name.is_empty() || name.contains('/') {
            return Err(target);
        }
        Ok(name)
    } else if target.is_empty() || target.contains('/') {
        Err(target)
    } else {
        Ok(target)
    }
}

/// Parse a target string into a service name.
///
/// Accepts bare names (`worker-a`) and explicit resource references
/// (`service/worker-a`). Rejects unknown resource type prefixes.
fn parse_target_name(target: &str) -> Result<&str, ComposeRuntimeError> {
    parse_target_name_raw(target).map_err(|target| ComposeRuntimeError::UnsupportedResourceType {
        target: target.to_owned(),
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

/// Validate that every target name exists among the discovered project
/// members. Returns the unknown names if any are missing.
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

    // Signal orchestrator only for project-wide down.
    let orchestrator = if targeted {
        None
    } else {
        let orch = find_active_orchestrator(&slug)?;
        if let Some(active) = orch.as_ref() {
            let signalled = signal_identity(&active.identity, SignalDelivery::Term)?;
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

    // Sweep late members only for project-wide down.
    if !targeted {
        sweep_late_members(&slug, &all_members, args).await?;
    }
    Ok(())
}

/// Best-effort liveness probe used by the wait loop. Returns `true` when
/// the recorded pid is still in the process table with a matching start
/// time, `false` otherwise. Probe failures (`/proc/<pid>/stat` transient
/// I/O, etc.) collapse to `true` — the conservative direction for a
/// caller deciding whether to keep escalating to SIGKILL.
fn record_pid_alive(handle: &ProcessHandle) -> bool {
    let PidFileState::Found(identity) = handle.record().pid_identity() else {
        return false;
    };
    process_is_alive_with_start_time(&identity).unwrap_or(true)
}

/// After the SIGKILL pass, re-walk the registry and SIGTERM/SIGKILL any
/// project members that appeared between the initial snapshot and the
/// orchestrator's death. Mirrors the way `docker compose down` reaps
/// containers the daemon spawned mid-shutdown — a stateless analogue
/// for our label-discovery model.
///
/// Bounded: with the orchestrator gone or being `SIGKILL`ed by
/// [`escalate_to_sigkill`], nothing new can spawn so we only need to
/// catch the late arrivals already on disk.
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
    // Collect failures but keep going so every latecomer is at least
    // attempted; surface the first error so `compose down` exits
    // non-zero rather than silently leaving a runaway alive (Codex
    // iter-11 Minor A1/A2). `cli_eprintln!` instead of `warn!` because
    // `run_compose_down` does not initialise `tracing`, so `warn!`
    // here would go nowhere.
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
        // The orchestrator is already gone, so there is no graceful
        // drain target left to honour: go straight to SIGKILL via
        // `kill()` (which records the terminal status transition and
        // delivers SIGKILL atomically). Tolerate the `IllegalTransition`
        // race where the record raced to terminal between our snapshot
        // and this call, and follow up with `force_kill` so the OS pid
        // is reaped even if the record was already terminal (Codex
        // iter-10 Minor A).
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

/// Final SIGKILL pass once the SIGTERM grace period elapsed. Mirrors
/// `docker compose down -t`: anything still alive past the timeout gets
/// `SIGKILL`, and we wait for the kernel to reap the orchestrator so the
/// caller's follow-up `kill(pid, 0)` (e.g. in tests) observes the pid
/// gone rather than zombie.
///
/// Distinguishes "we actually delivered SIGKILL" from "the process had
/// already exited" so operator-facing messages never falsely claim a
/// signal was sent to a dead process (Codex iter-14 Minor D1).
#[derive(Clone, Copy)]
enum KillOutcome {
    Delivered,
    AlreadyGone,
}

impl KillOutcome {
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
    // Collect per-target errors but keep escalating: stopping at the
    // first failure would leave the orchestrator alive and re-spawning
    // services (Codex iter-11 Major). Mirrors `docker compose down`,
    // which always sweeps every container before reporting failure.
    let mut first_error: Option<ComposeRuntimeError> = None;
    let mut record_error = |err: ComposeRuntimeError| {
        if first_error.is_none() {
            first_error = Some(err);
        }
    };

    for (id, service, handle) in still_alive {
        // Two cases land here:
        //
        //  1. Status is still non-terminal — `kill()` both delivers
        //     SIGKILL and flips the record. Tolerate the same TOCTOU
        //     race the SIGTERM loop above does: if the record raced
        //     past terminal between the deadline check and this call,
        //     `IllegalTransition { observed: terminal }` means the
        //     goal is already met.
        //  2. Status is *already* terminal but the OS pid is still
        //     alive (subprocess ignored SIGTERM). `kill()` would raise
        //     `IllegalTransition` and skip the SIGKILL we need; use
        //     `force_kill()` instead, which sends the signal *without*
        //     a status transition (Codex iter-9 Major 4).
        let status = match handle.refresh_status().await {
            Ok(s) => s,
            Err(err) => {
                // `refresh_status` reads the status file; failure here
                // does not mean the OS pid is gone, just that we can't
                // reconcile the record. Every entry in `still_alive`
                // already needed a SIGKILL attempt, so fall back to
                // `force_kill()` (which reads the pid file directly,
                // independent of the status file) so we never silently
                // skip a live process (Codex iter-12 Major B1). When
                // the fallback succeeds (`Ok(true)`), suppress the
                // refresh_status error — the kill ultimately worked,
                // and surfacing the reconcile error would cause
                // `compose down` to return `Err` and skip
                // `sweep_late_members` (Codex iter-13 Minor C1).
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
                        // pid file already gone or kernel reports the
                        // pid exited. Don't claim we killed it — but
                        // it's also no longer a live process, so the
                        // refresh_status error is moot for cleanup
                        // purposes. Don't promote the error.
                        if !args.quiet {
                            cli_eprintln!(
                                "project {slug:?}: service {service:?} ({id}) already gone (refresh_status failed but pid file shows exited)",
                                id = trunc_id(id, false)
                            );
                        }
                    }
                    Err(fk_err) => {
                        // Both reconcile and the fallback OS escalation
                        // failed: surface both to the caller via the
                        // accumulator so `compose down` exits non-zero.
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
        // `KillOutcome::Delivered` → SIGKILL was actually sent.
        // `KillOutcome::AlreadyGone` → process exited before we got
        // there (force_kill returned `Ok(false)`). Distinguish them so
        // we don't print "SIGKILL after Ns" for a process that was
        // already dead (Codex iter-14 Minor D1).
        let kill_outcome: Result<KillOutcome, ComposeRuntimeError> = if status.is_terminal() {
            // `force_kill` errors must surface: this branch is the only
            // OS-level escalation left for a terminal-record-but-pid-alive
            // process, and silently dropping `Err` would let `compose
            // down` return `Ok(())` while a service stays alive (Codex
            // iter-10 Major).
            handle
                .force_kill()
                .map(KillOutcome::from_force_kill)
                .map_err(Into::into)
        } else {
            match handle.kill().await {
                Ok(_) => Ok(KillOutcome::Delivered),
                Err(ProcessError::IllegalTransition {
                    observed: Some(o), ..
                }) if o.is_terminal() => {
                    // Race window: status flipped to terminal between
                    // `refresh_status` above and this `kill`. Follow
                    // up with `force_kill` so we never silently skip
                    // an OS-level escalation.
                    handle
                        .force_kill()
                        .map(KillOutcome::from_force_kill)
                        .map_err(Into::into)
                }
                Err(err) => Err(err.into()),
            }
        };
        match kill_outcome {
            Err(err) => {
                cli_eprintln!(
                    "project {slug:?}: SIGKILL service {service:?} ({id}) failed: {err}",
                    id = trunc_id(id, false)
                );
                record_error(err);
            }
            Ok(KillOutcome::Delivered) => {
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: SIGKILL service {service:?} ({id}) after {timeout_s}s",
                        timeout_s = args.timeout,
                        id = trunc_id(id, false)
                    );
                }
            }
            Ok(KillOutcome::AlreadyGone) => {
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: service {service:?} ({id}) already exited before SIGKILL",
                        id = trunc_id(id, false)
                    );
                }
            }
        }
    }

    // Always attempt the orchestrator escalation, even if some services
    // above failed: leaving the orchestrator alive lets it spawn new
    // services and undermines the entire `compose down`.
    if let Some(identity) = orchestrator_identity {
        match signal_identity(identity, SignalDelivery::Kill) {
            Ok(true) => {
                let pid = identity.pid.as_raw();
                if !args.quiet {
                    cli_eprintln!(
                        "project {slug:?}: SIGKILL orchestrator pid {pid} after {timeout_s}s",
                        timeout_s = args.timeout,
                    );
                }
                let kill_deadline = Instant::now() + Duration::from_secs(2);
                // `unwrap_or(false)` — once we've delivered SIGKILL, a
                // probe failure is no reason to spin until the deadline.
                // Treat unknown as "exited" and let the caller move on
                // (Codex iter-9 Minor 2).
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

/// Run `iter validate` against either an Iterfile or a compose.iter,
/// auto-detected by basename. Used by [`crate::dispatch::run_validate`].
///
/// # Errors
///
/// Forwarded from the underlying loader / validator.
pub fn validate_path_autodetect(
    path: Option<&Path>,
    format: ValidateFormat,
) -> Result<(), ValidateAutodetectError> {
    let resolved = match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("Iterfile"),
    };
    if is_compose_filename(&resolved) {
        run_compose_validate(&ComposeValidateArgs {
            file: Some(resolved.clone()),
            format,
        })
        .map_err(|source| ValidateAutodetectError::Compose {
            path: resolved,
            source: Box::new(source),
        })
    } else {
        let loaded = load_iterfile(Some(&resolved))?;
        match format {
            ValidateFormat::Text => cli_println!("OK"),
            ValidateFormat::Json => {
                let envelope = ValidateOk {
                    ok: true,
                    summary: ValidateSummary {
                        queues: usize::from(loaded.iterfile.queue.is_some()),
                        services: 1,
                        triggers: 0,
                    },
                };
                print_json_compact(&envelope).map_err(ValidateAutodetectError::JsonSerialize)?;
            }
        }
        Ok(())
    }
}
