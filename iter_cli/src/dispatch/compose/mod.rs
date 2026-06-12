//! `iter compose` — `up`, `validate`, `config`, `ls`, `ps`, `down` dispatchers.
//!
//! Each public function here corresponds to one variant of
//! [`crate::cli::ComposeCmd`]. The heavy lifting lives in the CLI's compose
//! run modules ([`crate::compose`]) — these handlers are thin wrappers that
//! resolve the file path, install the shutdown handler, render diagnostics,
//! and forward the result.
//!
//! `config` is the static plan listing (queues / services / triggers parsed
//! from `compose.iter`). `ls` / `ps` / `down` operate on the **runtime**
//! state reconstructed from `iter.compose.*` labels stamped onto child
//! runners — mirroring how `docker compose ps` reads container labels.

mod down;
mod inspect;
mod up;

use std::path::{Path, PathBuf};

use crate::{ComposeError, DEFAULT_COMPOSE_FILE, ProjectSlugError, project_slug};
use thiserror::Error;

use crate::output::{IntoExitCode, exit_codes};

pub use down::run_compose_down;
pub use inspect::{
    run_compose_config, run_compose_ls, run_compose_ps, run_compose_validate,
    validate_path_autodetect,
};
pub use up::run_compose_up;

/// Errors produced by `iter compose up`.
///
/// Heavy underlying error types (`ProcessError`, `ComposeError`) are boxed
/// so the enum stays inside clippy's `result_large_err` budget.
#[derive(Debug, Error)]
pub enum ComposeUpError {
    /// Loading or building the compose file failed.
    #[error(transparent)]
    Compose(Box<ComposeError>),
    /// Installing the interrupt (`SIGINT`/`SIGTERM`) handler failed.
    #[error("installing interrupt handler: {0}")]
    Signals(#[source] std::io::Error),
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
    OrchestratorIdentity(#[source] Box<iter_core::process::ProcessError>),
    /// Scanning the registry for an existing project orchestrator failed.
    #[error("scanning registry for active project: {0}")]
    Discovery(#[source] Box<crate::DiscoveryError>),
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
    TargetedSpawn(#[from] crate::TargetedSpawnError),
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
    ProjectLock(#[from] crate::ProjectLockError),
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

impl From<crate::DiscoveryError> for ComposeUpError {
    fn from(value: crate::DiscoveryError) -> Self {
        Self::Discovery(Box::new(value))
    }
}

impl IntoExitCode for ComposeUpError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Compose(e) => compose_error_exit_code(e),
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
    Discovery(#[from] crate::DiscoveryError),
    /// Resolving the project slug for `ps` / `down` failed.
    #[error(transparent)]
    ProjectSlug(Box<ProjectSlugError>),
    /// Opening a runner handle to deliver `SIGTERM`/`SIGKILL` failed.
    #[error(transparent)]
    Process(#[from] iter_core::process::ProcessError),
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
        | ComposeError::Start { .. }
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
    Load(#[from] crate::dispatch::load::LoadError),
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

fn resolve_compose_path(path: Option<&Path>) -> PathBuf {
    match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(DEFAULT_COMPOSE_FILE),
    }
}

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
