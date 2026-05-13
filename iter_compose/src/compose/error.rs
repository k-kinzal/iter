//! Error types for compose load / build / run.

use std::path::{Path, PathBuf};

use iter_core::{BuilderError, RunnerExitError, TemplateError};
use iter_language::Diagnostic;
use thiserror::Error;

use iter_core::process::{ProcessError, ProcessStatus, SpawnError};

use crate::agent::AgentBuildError;
use crate::arg::ArgError;
use crate::prompt::PromptBuildError;
use crate::queue::QueueBuildError;

/// Errors produced while loading or building a `compose.iter` file.
#[derive(Debug, Error)]
pub enum ComposeError {
    /// Reading a compose / iterfile from disk failed.
    #[error("reading {path}: {source}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The parser produced one or more error-severity diagnostics.
    #[error("{rendered}")]
    Parse {
        /// Pre-rendered diagnostic text.
        rendered: String,
    },
    /// A `service { build = ... }` target declared its own `queue` block.
    #[error(
        "service '{service}' build target '{}' declares a queue, but \
         compose-managed services must inherit queue from compose.iter. \
         Remove the queue section from {} or define this service inline.",
        path.display(),
        path.display()
    )]
    BuildTargetHasQueue {
        /// Service name.
        service: String,
        /// Resolved Iterfile path.
        path: PathBuf,
    },
    /// A required section is missing from a service definition.
    #[error("service `{service}` is missing the `{section}` section")]
    ServiceMissingSection {
        /// Service name.
        service: String,
        /// Missing section name (`workspace` / `agent` / `runner`).
        section: &'static str,
    },
    /// Internal invariant: a queue reference was unresolved at build time.
    #[error("internal: compose service `{0}` has unresolved queue ref")]
    UnresolvedServiceQueue(String),
    /// Internal invariant: queue reference left anonymous after lowering.
    #[error("internal: queue reference is unresolved (semantic layer should have set it)")]
    UnresolvedAnonymousQueueRef,
    /// A service named a queue that was not declared.
    #[error("queue `{0}` is not declared in this compose.iter")]
    UnknownQueue(String),
    /// Building a queue declaration failed.
    #[error("building queue `{name}`: {source}")]
    QueueBuild {
        /// Queue name from the compose file.
        name: String,
        /// Underlying queue-build error.
        #[source]
        source: QueueBuildError,
    },
    /// Building a queue from a connection URL failed.
    #[error(transparent)]
    Queue(#[from] QueueBuildError),
    /// Resolving Iterfile `arg` declarations for a service failed.
    #[error("resolving args for service `{service}`: {source}")]
    ArgResolve {
        /// Service name.
        service: String,
        /// Underlying arg-resolution error.
        #[source]
        source: ArgError,
    },
    /// Building an agent for a service failed.
    #[error("building service `{service}`: {source}")]
    AgentBuild {
        /// Service name.
        service: String,
        /// Underlying agent-build error.
        #[source]
        source: AgentBuildError,
    },
    /// Building the prompt selector for a service failed.
    #[error("building service `{service}`: {source}")]
    PromptBuild {
        /// Service name.
        service: String,
        /// Underlying prompt-build error.
        #[source]
        source: PromptBuildError,
    },
    /// Compiling an `on <event>` handler template failed.
    #[error("building service `{service}`: invalid event handler template: {source}")]
    EventTemplate {
        /// Service name.
        service: String,
        /// Underlying template-compile error.
        #[source]
        source: TemplateError,
    },
    /// A `--service NAME` selector named a service that does not exist
    /// in the compose file.
    #[error("compose file has no service named `{0}`")]
    UnknownService(String),
    /// `RunnerBuilder::build` rejected the assembled configuration.
    #[error("building service `{service}`: {source}")]
    Builder {
        /// Service name.
        service: String,
        /// Underlying builder error.
        #[source]
        source: BuilderError,
    },
    /// The compose file declares zero services. iter's runner-as-unit-of-
    /// exploration model has no meaning for a service-less compose, and
    /// `iter compose ls`/`ps` would never surface such a project. We
    /// reject at build time so users see the problem before `up`.
    #[error(
        "compose file {} declares no services; \
         compose-managed projects must define at least one `service` block",
        path.display()
    )]
    NoServices {
        /// Compose file path that triggered the failure.
        path: PathBuf,
    },
    /// A `compose` block creates a circular import chain.
    #[error(
        "circular compose import: {} is already in the import chain: {}",
        path.display(),
        chain.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" → ")
    )]
    CircularComposeImport {
        /// The compose file that was encountered a second time.
        path: PathBuf,
        /// The chain of compose files leading up to the cycle.
        chain: Vec<PathBuf>,
    },
    /// A name declared in a child compose file collides with a name in the
    /// parent compose file or another child.
    #[error(
        "{kind} name `{name}` from {} collides with a declaration in {}",
        child_path.display(),
        parent_path.display()
    )]
    ComposeNameCollision {
        /// The kind of element that collided (queue / service / trigger).
        kind: &'static str,
        /// The conflicting name.
        name: String,
        /// Compose file where the child element is defined.
        child_path: PathBuf,
        /// Compose file where the conflicting parent element is defined.
        parent_path: PathBuf,
    },
    /// A `compose` block's `queues` override references a queue that does
    /// not exist in the child compose file.
    #[error(
        "compose block references child queue `{queue_name}` in override, \
         but {} does not declare it",
        compose_path.display()
    )]
    UnknownChildQueue {
        /// Path of the child compose file.
        compose_path: PathBuf,
        /// The queue name that was referenced in the override.
        queue_name: String,
    },
    /// A `compose` block's `services` override references a service that
    /// does not exist in the child compose file.
    #[error(
        "compose block references child service `{service_name}` in override, \
         but {} does not declare it",
        compose_path.display()
    )]
    UnknownChildService {
        /// Path of the child compose file.
        compose_path: PathBuf,
        /// The service name that was referenced.
        service_name: String,
    },
    /// A `compose` block's `triggers` override references a trigger that
    /// does not exist in the child compose file.
    #[error(
        "compose block references child trigger `{trigger_name}` in override, \
         but {} does not declare it",
        compose_path.display()
    )]
    UnknownChildTrigger {
        /// Path of the child compose file.
        compose_path: PathBuf,
        /// The trigger name that was referenced.
        trigger_name: String,
    },
    /// A trigger kind is not supported in the compose runtime.
    #[error("trigger `{trigger_name}` uses unsupported kind `{kind}` in compose runtime")]
    UnsupportedTriggerKind {
        /// The trigger name.
        trigger_name: String,
        /// The unsupported trigger kind.
        kind: String,
    },
}

impl ComposeError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn parse(path: &Path, source: &str, diags: &[Diagnostic]) -> Self {
        if diags.is_empty() {
            return Self::Parse {
                rendered: format!("{} failed to parse with no diagnostics", path.display()),
            };
        }
        let label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("compose");
        let mut rendered = String::new();
        for diag in diags {
            rendered.push_str(&diag.report(label, source));
            rendered.push('\n');
        }
        Self::Parse {
            rendered: rendered.trim_end().to_owned(),
        }
    }
}

/// Errors a single compose service can surface from its run task.
///
/// Lifecycle and Builder errors mean the service never ran; Runner is
/// the runner's own exit error; `FinalizeStatus` surfaces a non-terminal
/// registry record left behind after a clean run.
#[derive(Debug, Error)]
pub enum ServiceRunError {
    /// Foreground process registry bootstrap failed.
    #[error(transparent)]
    Lifecycle(#[from] crate::process_lifecycle::LifecycleError),
    /// `RunnerBuilder::build` rejected the assembled configuration at
    /// run time (after observer wiring).
    #[error(transparent)]
    Builder(#[from] BuilderError),
    /// The runner exited with an error.
    #[error(transparent)]
    Runner(#[from] RunnerExitError),
    /// Runner finished cleanly but the registry record could not be
    /// flipped to a terminal state.
    #[error("finalize failed to write terminal status: {0}")]
    FinalizeStatus(#[source] ProcessError),
}

/// Errors produced by [`super::spawn_targeted_service`].
#[derive(Debug, Error)]
pub enum TargetedSpawnError {
    /// The named service does not exist in the plan. Defensive guard for
    /// callers that skip pre-validation; `run_compose_up_targeted` validates
    /// names before calling `spawn_targeted_service`, so this is unreachable
    /// through that path.
    #[error("compose file has no service named `{0}`")]
    UnknownService(String),
    /// The service's queue is not URL-addressable; cross-process restart
    /// requires `file://`, `redis://`, or another addressable backend.
    #[error(
        "service `{service}` uses a non-addressable queue; targeted restart \
         requires a URL-addressable queue backend (file://, redis://, etc.)"
    )]
    NonAddressable {
        /// The service whose queue lacks a URL form.
        service: String,
    },
    /// Opening the process registry failed.
    #[error("opening process registry: {0}")]
    OpenRegistry(#[source] ProcessError),
    /// Locating the current `iter` binary failed.
    #[error("locating iter binary: {0}")]
    Binary(#[source] std::io::Error),
    /// Spawning the service subprocess failed.
    #[error("spawning service subprocess: {0}")]
    Spawn(#[source] SpawnError),
}

/// Errors a subprocess-spawned service can surface.
///
/// The `Binary {…}` arm carries a runtime-resolved program path
/// (`current_exe()`) rather than a static basename so failure
/// diagnostics name the exact `iter` executable that failed to fork.
#[derive(Debug, Error)]
pub enum ServiceSubprocessError {
    /// Opening the process registry failed.
    #[error("opening process registry: {0}")]
    OpenRegistry(#[source] ProcessError),
    /// Locating the current `iter` binary failed.
    #[error("locating iter binary: {0}")]
    Binary(#[source] std::io::Error),
    /// `spawn_detached` failed to fork the service child.
    #[error("spawning service subprocess: {0}")]
    Spawn(#[source] SpawnError),
    /// Opening the child's process handle failed.
    #[error("opening service handle: {0}")]
    OpenHandle(#[source] ProcessError),
    /// Reading the child's terminal status failed.
    #[error("reading service status: {0}")]
    Status(#[source] ProcessError),
    /// The child exited in a non-`Stopped` terminal state.
    #[error("service subprocess exited with status {0:?}")]
    NonZeroExit(ProcessStatus),
}
