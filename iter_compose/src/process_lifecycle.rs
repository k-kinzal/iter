//! Shared process-registry bootstrap used by both `iter run` and
//! `iter compose up`.
//!
//! Both code paths take a runner that is about to start, register a
//! foreground record under `~/.iter/proc/<id>/`, wire stdio + observer
//! side files, and finalise the record with a derived
//! [`ProcessTerminationReason`] when the runner exits. This module
//! collects the bootstrap, finalize-reason derivation, and best-effort
//! finalize logging into one place so that compose-managed services
//! show up in `iter ps` exactly the same way `iter run` records do.
//!
//! The adopted (`--process-id`) bootstrap is iterfile-specific and
//! lives in `iter_compose::iterfile` — only the foreground path is
//! shared here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use iter_core::process::registry::MetadataDraft;
use iter_core::process::{
    AdoptError, FinalizeReport, LifecycleObserver, ObserverError, ProcessError, ProcessId,
    ProcessRegistry, ProcessRuntime, ProcessStatus, ProcessTerminationReason, RegisterError,
    ShutdownController, StartupError, StdioPolicy, StdioSupervisor, adopt_from_argv,
    current_identity,
};
use thiserror::Error;
use tracing::warn;

/// Bookkeeping recorded into the registry entry's `meta.json`.
///
/// All fields are informational: the bootstrap stores them verbatim and
/// does not interpret them. The CLI dispatch layer is the source of
/// truth for argv shape and flag semantics.
#[derive(Clone)]
pub struct RunRecordMetadata {
    /// CLI-shaped argv recorded for `iter inspect`.
    pub argv: Vec<String>,
    /// Subcommand verb (e.g. `"run"` or `"compose up"`).
    pub subcommand: String,
    /// `--debug` flag value.
    pub debug: bool,
}

/// Errors produced by [`bootstrap_adopted`].
#[derive(Debug, Error)]
pub enum AdoptedBootstrapError {
    /// Opening the process registry failed.
    #[error("opening process registry: {0}")]
    RegistryOpen(#[source] ProcessError),
    /// Adopting the parent-allocated record failed.
    #[error("adopting process {id}: {source}")]
    Adopt {
        /// Adopted process id.
        id: ProcessId,
        /// Underlying error.
        #[source]
        source: AdoptError,
    },
    /// Wiring the runtime side files (observer, stdio supervisor, shutdown
    /// controller) failed after adoption succeeded.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
}

/// Errors produced by [`bootstrap_foreground`].
#[derive(Debug, Error)]
pub enum LifecycleError {
    /// Installing shutdown signal handlers failed.
    #[error("installing shutdown signal handlers: {0}")]
    InstallSignalHandlers(#[source] std::io::Error),
    /// Opening the per-process lifecycle observer failed.
    #[error("opening lifecycle observer: {0}")]
    Observer(#[source] ObserverError),
    /// Opening the stdio supervisor failed.
    #[error("opening stdio supervisor: {0}")]
    Stdio(#[source] std::io::Error),
    /// Registering a foreground process record failed.
    #[error("registering process {name}: {source}")]
    Register {
        /// Process name.
        name: String,
        /// Underlying error.
        #[source]
        source: RegisterError,
    },
    /// Collecting the current process identity failed.
    #[error("collecting current process identity: {0}")]
    Identity(#[source] ProcessError),
    /// Performing the locked initial write of the foreground record failed.
    #[error("locked_initial_write: {0}")]
    LockedInitialWrite(#[source] StartupError),
}

/// Register a foreground process record and wire up its side files.
///
/// Returns `Ok(None)` when the process registry is unavailable (e.g.
/// read-only `$HOME`); the caller should still run the underlying work
/// without registry visibility. Other failures bubble up so the caller
/// can surface them.
///
/// # Errors
///
/// Any of the [`LifecycleError`] variants.
pub(crate) async fn bootstrap_foreground(
    name: &str,
    iterfile_path: &Path,
    metadata: &RunRecordMetadata,
    labels: Option<BTreeMap<String, String>>,
) -> Result<Option<ProcessRuntime>, LifecycleError> {
    let registry = match ProcessRegistry::open_default() {
        Ok(r) => r,
        Err(err) => {
            warn!(
                error = %err,
                name = %name,
                "process registry unavailable; foreground run will not be recorded \
                (iter ps / logs / stop will not see this run)"
            );
            return Ok(None);
        }
    };

    let draft = MetadataDraft {
        iterfile: iterfile_path.to_owned(),
        subcommand: metadata.subcommand.clone(),
        started_at: Utc::now(),
        args: metadata.argv.clone(),
        env: Vec::new(),
        debug: metadata.debug,
        parent_id: None,
        labels: labels.unwrap_or_default(),
    };
    let (session, lock) = registry
        .register_foreground(name, draft)
        .await
        .map_err(|source| LifecycleError::Register {
            name: name.to_owned(),
            source,
        })?;
    // Holding the fd would pin the exclusive flock and serialize
    // subsequent runs with the same name behind us instead of letting
    // them see `AlreadyExists`. The on-disk lock body persists as the
    // registry entry until `Handle::remove` unlinks it.
    drop(lock);

    // The foreground process inherits the user's terminal on fd 1/2.
    // `LogOnly` would route the worker's stdio into `log.ndjson` only,
    // which is wrong for an interactive `iter run` invocation.
    let stdio_policy = StdioPolicy::Passthrough;

    let (observer, stdio, shutdown) =
        match wire_runtime_pieces(session.paths().dir(), stdio_policy).await {
            Ok(triple) => triple,
            Err(err) => {
                if let Err(transition_err) = session
                    .status_file()
                    .transition(ProcessStatus::Initializing, ProcessStatus::Failed)
                    .await
                {
                    warn!(
                        error = %transition_err,
                        "failed to mark foreground process Failed after bootstrap error",
                    );
                }
                return Err(err);
            }
        };

    let identity = match current_identity() {
        Ok(id) => id,
        Err(err) => {
            if let Err(transition_err) = session
                .status_file()
                .transition(ProcessStatus::Initializing, ProcessStatus::Failed)
                .await
            {
                warn!(
                    error = %transition_err,
                    "failed to mark foreground process Failed after current_identity error",
                );
            }
            return Err(LifecycleError::Identity(err));
        }
    };
    if let Err(err) = session
        .status_file()
        .clone()
        .locked_initial_write(identity, session.paths().clone())
        .await
    {
        if let Err(transition_err) = session
            .status_file()
            .transition(ProcessStatus::Initializing, ProcessStatus::Failed)
            .await
        {
            warn!(
                error = %transition_err,
                "failed to mark foreground process Failed after locked_initial_write error",
            );
        }
        return Err(LifecycleError::LockedInitialWrite(err));
    }

    Ok(Some(ProcessRuntime::new(
        session, shutdown, observer, stdio,
    )))
}

/// Adopt a parent-allocated registry record and wire up its side files.
///
/// This is the shared adoption entry point used by both `iter run
/// --process-id` (via [`crate::iterfile::handle`]) and `iter compose up
/// --process-id` (via the CLI dispatch). The parent (`spawn_detached`)
/// has already created `~/.iter/proc/<id>/`, written `meta.json`,
/// `bootstrap_token`, and bound the child's fd 1/2 to `/dev/null`. The
/// per-process `log.ndjson` is opened from inside the adopted child by
/// [`StdioPolicy::LogOnly`] and is the only path that produces records
/// in that file.
/// This helper takes care of:
///
/// 1. Calling [`adopt_from_argv`], which atomically flips the record
///    `Initializing → Running`, publishes the pid file, and deletes the
///    bootstrap token.
/// 2. Wiring the observer + stdio supervisor + signal-driven shutdown
///    controller in [`StdioPolicy::LogOnly`] mode (consistent with the
///    on-disk fd layout the parent set up).
/// 3. Returning a [`ProcessRuntime`] the caller is responsible for
///    finalising on exit so the record reaches a terminal state.
///
/// On any post-adoption failure the record is best-effort transitioned
/// `Running → Failed` so it does not dangle.
///
/// # Errors
///
/// Any [`AdoptedBootstrapError`] variant.
pub async fn bootstrap_adopted(
    process_id: ProcessId,
) -> Result<ProcessRuntime, AdoptedBootstrapError> {
    let registry = ProcessRegistry::open_default().map_err(AdoptedBootstrapError::RegistryOpen)?;
    let session = adopt_from_argv(registry.proc_root(), process_id)
        .await
        .map_err(|source| AdoptedBootstrapError::Adopt {
            id: process_id,
            source,
        })?;

    let stdio_policy = StdioPolicy::LogOnly {
        log_dir: session.paths().dir().to_owned(),
    };

    match wire_runtime_pieces(session.paths().dir(), stdio_policy).await {
        Ok((observer, stdio, shutdown)) => {
            Ok(ProcessRuntime::new(session, shutdown, observer, stdio))
        }
        Err(err) => {
            if let Err(transition_err) = session
                .status_file()
                .transition(ProcessStatus::Running, ProcessStatus::Failed)
                .await
            {
                warn!(
                    error = %transition_err,
                    "failed to mark adopted process Failed after bootstrap error",
                );
            }
            Err(AdoptedBootstrapError::Lifecycle(err))
        }
    }
}

/// Open the per-process side files (lifecycle observer, stdio
/// supervisor, signal-handling shutdown controller) shared by both
/// adopted and foreground runs.
///
/// `pub(crate)` so [`crate::iterfile`]'s adopted-mode bootstrap can
/// reuse the same helper.
///
/// # Errors
///
/// Any of the I/O or observer failures wrapped in [`LifecycleError`].
pub(crate) async fn wire_runtime_pieces(
    dir: &Path,
    stdio_policy: StdioPolicy,
) -> Result<(Arc<LifecycleObserver>, StdioSupervisor, ShutdownController), LifecycleError> {
    // Construct the stdio supervisor first so the lifecycle observer
    // can be wired with a back-pressured `LogJsonSender` into the same
    // `log.ndjson` pipeline. Without this ordering, the lifecycle
    // writer task would have to fall back to the best-effort tracing
    // path (`try_send_line`) for every record — at finalize time, a
    // full `LogJsonSink` channel would silently drop the last
    // `AgentFinished` / `WorkspaceTearDown` events instead of
    // back-pressuring the runner.
    let stdio = StdioSupervisor::new(stdio_policy)
        .await
        .map_err(LifecycleError::Stdio)?;
    let observer = Arc::new(
        LifecycleObserver::open_in(dir, stdio.log_sender())
            .await
            .map_err(LifecycleError::Observer)?,
    );
    let shutdown = ShutdownController::new();
    shutdown
        .install_signal_handlers()
        .map_err(LifecycleError::InstallSignalHandlers)?;
    Ok((observer, stdio, shutdown))
}

/// Log non-clean entries from a [`FinalizeReport`].
pub fn log_finalize_report(report: &FinalizeReport) {
    if report.is_clean() {
        return;
    }
    if let Some(err) = report.status_write_error.as_ref() {
        warn!(error = %err, "finalize failed to write terminal status");
    }
    for err in &report.stdio_errors {
        warn!(error = %err, "finalize stdio drain error");
    }
    for err in &report.observer_errors {
        warn!(error = %err, "finalize observer drain error");
    }
}

/// True if the finalize error means the on-disk record still needs a
/// terminal status written. False for the `iter stop`/`iter kill` race:
/// the operator's command writes `Killed` synchronously, then the
/// runner reaches its own `finalize` and observes the record is already
/// terminal. The on-disk state is correct in that case, so we do not
/// want to flip the user-visible exit code on what is effectively a
/// no-op.
#[must_use]
pub fn leaves_record_non_terminal(err: &ProcessError) -> bool {
    !matches!(
        err,
        ProcessError::IllegalTransition {
            observed: Some(observed),
            ..
        } if observed.is_terminal()
    )
}

/// Derive a [`ProcessTerminationReason`] from a runner outcome and
/// shutdown state.
///
/// The shutdown controller wins when set (signals already recorded a
/// reason). Otherwise `Completed` for clean returns and
/// `RunnerError(msg)` for failures.
#[must_use]
pub fn derive_finalize_reason(
    runner_failure_message: Option<String>,
    shutdown: &ShutdownController,
) -> ProcessTerminationReason {
    if let Some(reason) = shutdown.reason_taken() {
        return reason;
    }
    match runner_failure_message {
        None => ProcessTerminationReason::Completed,
        Some(msg) => ProcessTerminationReason::RunnerError(msg.into()),
    }
}
