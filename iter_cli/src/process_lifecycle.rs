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
//! lives in `crate::iterfile` — only the foreground path is
//! shared here.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use iter_core::process::registry::MetadataDraft;
use iter_core::process::{
    AdoptError, FinalizeReport, Interrupt, LifecycleObserver, ObserverError, OutputPolicy,
    ProcessError, ProcessId, ProcessOutput, ProcessRegistry, ProcessRuntime, ProcessStatus,
    RegisterError, ShutdownIntent, StartupError, adopt_from_argv, current_identity, open_output,
    spawn_interrupt_listener,
};
use thiserror::Error;
use tracing::warn;

/// Boxed error trait object carried inside
/// [`ProcessTerminationReason::RunnerError`].
pub type BoxError = Box<dyn StdError + Send + Sync + 'static>;

/// Why the process exited its main run loop — the run record's
/// termination classification.
///
/// This taxonomy belongs to the run record (the operator's durable
/// memory under `~/.iter/proc`), so it lives here with the record's
/// finalisation rather than in core: core's `process::interrupt` records
/// the *intent* to shut down ([`ShutdownIntent`]) and nothing else.
///
/// Per rev17 §J1, this is the input [`terminal_status_for`] needs in
/// order to pick a terminal [`ProcessStatus`]:
///
/// | reason            | terminal status |
/// |-------------------|-----------------|
/// | `Completed`       | `Stopped`       |
/// | `RunnerError(_)`  | `Failed`        |
/// | `SignalTerm`      | `Killed`        |
/// | `SignalInt`       | `Killed`        |
///
/// A panicking runner is deliberately absent: the runner is awaited
/// inline, so a panic unwinds past the finalize path and the record is
/// promoted to a terminal state by the registry reconciler
/// (`refresh_status`) instead.
#[derive(Debug)]
pub enum ProcessTerminationReason {
    /// Runner returned `Ok(_)` from its main loop.
    Completed,
    /// Runner returned `Err(_)` from its main loop.
    RunnerError(BoxError),
    /// `SIGTERM` was observed before the runner returned.
    SignalTerm,
    /// `SIGINT` (Ctrl-C) was observed before the runner returned.
    SignalInt,
}

/// Map a [`ProcessTerminationReason`] to its terminal [`ProcessStatus`]
/// per the rev17 §J1 table.
#[must_use]
pub fn terminal_status_for(reason: &ProcessTerminationReason) -> ProcessStatus {
    match reason {
        ProcessTerminationReason::Completed => ProcessStatus::Stopped,
        ProcessTerminationReason::RunnerError(_) => ProcessStatus::Failed,
        ProcessTerminationReason::SignalTerm | ProcessTerminationReason::SignalInt => {
            ProcessStatus::Killed
        }
    }
}

/// Reason-recording layer over a [`ShutdownIntent`].
///
/// Core's interrupt module owns the OS-signal → cancellation mirror and
/// records intent only; classifying *why* the run ended belongs to the
/// run record and therefore lives here, with its operator. Cloning is
/// cheap (`Arc` + token internals) and shares both the token and the
/// reason slot. The first [`Self::cancel`] wins; the recorded reason is
/// what [`derive_finalize_reason`] feeds into [`terminal_status_for`]
/// when the record is finalised.
#[derive(Debug, Clone)]
pub struct TerminationRecorder {
    intent: ShutdownIntent,
    reason: Arc<Mutex<Option<ProcessTerminationReason>>>,
}

impl TerminationRecorder {
    /// Create a recorder around a fresh [`ShutdownIntent`] and an empty
    /// reason slot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            intent: ShutdownIntent::new(),
            reason: Arc::new(Mutex::new(None)),
        }
    }

    /// Return a clone of the underlying [`ShutdownIntent`]. Cheap — the
    /// token internals are reference-counted and the clone shares them.
    #[must_use]
    pub fn intent(&self) -> ShutdownIntent {
        self.intent.clone()
    }

    /// Trigger shutdown and record the reason.
    ///
    /// First call wins: subsequent invocations leave the recorded reason
    /// untouched and only ensure the underlying token is fired
    /// (idempotent).
    pub fn cancel(&self, reason: ProcessTerminationReason) {
        // `Mutex::lock` only fails if a previous holder panicked. Since
        // every code path here only takes the lock long enough to
        // `take`/`insert`, that should not happen — but if it does we
        // recover the inner state and proceed: we'd rather record a
        // best-effort reason than block on a poisoned shutdown.
        let mut slot = match self.reason.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if slot.is_none() {
            *slot = Some(reason);
        }
        drop(slot);
        self.intent.cancel();
    }

    /// Take the recorded reason, leaving the slot empty.
    ///
    /// Returns `None` until the first [`Self::cancel`] (or
    /// signal-handler trigger) records one.
    #[must_use]
    pub fn reason_taken(&self) -> Option<ProcessTerminationReason> {
        let mut slot = match self.reason.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        slot.take()
    }

    /// Mirror `SIGINT`/`SIGTERM` onto this recorder.
    ///
    /// Spawns core's interrupt listener and records [`SignalTerm`] or
    /// [`SignalInt`] when a signal fires, firing the underlying token.
    /// The task self-terminates if the token fires for any other reason.
    /// On non-unix targets only `Ctrl-C` is wired and it records
    /// [`SignalInt`].
    ///
    /// # Errors
    ///
    /// Returns the underlying [`std::io::Error`] if the unix signal
    /// listeners cannot be installed.
    ///
    /// [`SignalTerm`]: ProcessTerminationReason::SignalTerm
    /// [`SignalInt`]: ProcessTerminationReason::SignalInt
    pub fn install_signal_handlers(&self) -> std::io::Result<()> {
        let recorder = self.clone();
        spawn_interrupt_listener(self.intent.token(), move |which| {
            let reason = match which {
                Interrupt::Terminate => ProcessTerminationReason::SignalTerm,
                Interrupt::Interrupt => ProcessTerminationReason::SignalInt,
            };
            recorder.cancel(reason);
        })
    }
}

impl Default for TerminationRecorder {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// Wiring the runtime side files (observer, output sink, termination
    /// recorder) failed after adoption succeeded.
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
    /// Opening the output sink failed.
    #[error("opening output sink: {0}")]
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
) -> Result<Option<(ProcessRuntime, TerminationRecorder)>, LifecycleError> {
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
    let output_policy = OutputPolicy::Passthrough;

    let (observer, output, termination) =
        match wire_runtime_pieces(session.paths().dir(), &output_policy).await {
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

    Ok(Some((
        ProcessRuntime::new(session, termination.intent(), observer, output),
        termination,
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
/// [`OutputPolicy::LogOnly`] and is the only path that produces records
/// in that file.
/// This helper takes care of:
///
/// 1. Calling [`adopt_from_argv`], which atomically flips the record
///    `Initializing → Running`, publishes the pid file, and deletes the
///    bootstrap token.
/// 2. Wiring the observer + output sink + signal-driven
///    [`TerminationRecorder`] in [`OutputPolicy::LogOnly`] mode
///    (consistent with the on-disk fd layout the parent set up).
/// 3. Returning a [`ProcessRuntime`] (plus its recorder) the caller is
///    responsible for finalising on exit so the record reaches a
///    terminal state.
///
/// On any post-adoption failure the record is best-effort transitioned
/// `Running → Failed` so it does not dangle.
///
/// # Errors
///
/// Any [`AdoptedBootstrapError`] variant.
pub async fn bootstrap_adopted(
    process_id: ProcessId,
) -> Result<(ProcessRuntime, TerminationRecorder), AdoptedBootstrapError> {
    let registry = ProcessRegistry::open_default().map_err(AdoptedBootstrapError::RegistryOpen)?;
    let session = adopt_from_argv(registry.proc_root(), process_id)
        .await
        .map_err(|source| AdoptedBootstrapError::Adopt {
            id: process_id,
            source,
        })?;

    let output_policy = OutputPolicy::LogOnly {
        log_dir: session.paths().dir().to_owned(),
    };

    match wire_runtime_pieces(session.paths().dir(), &output_policy).await {
        Ok((observer, output, termination)) => Ok((
            ProcessRuntime::new(session, termination.intent(), observer, output),
            termination,
        )),
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

/// Open the per-process side files (lifecycle observer, output sink,
/// signal-handling [`TerminationRecorder`]) shared by both adopted and
/// foreground runs.
///
/// `pub(crate)` so [`crate::iterfile`]'s adopted-mode bootstrap can
/// reuse the same helper.
///
/// # Errors
///
/// Any of the I/O or observer failures wrapped in [`LifecycleError`].
pub(crate) async fn wire_runtime_pieces(
    dir: &Path,
    output_policy: &OutputPolicy,
) -> Result<(Arc<LifecycleObserver>, ProcessOutput, TerminationRecorder), LifecycleError> {
    let output = open_output(output_policy)
        .await
        .map_err(LifecycleError::Stdio)?;
    let observer = Arc::new(
        LifecycleObserver::open_in(dir, output.log_sender())
            .await
            .map_err(LifecycleError::Observer)?,
    );
    let termination = TerminationRecorder::new();
    termination
        .install_signal_handlers()
        .map_err(LifecycleError::InstallSignalHandlers)?;
    Ok((observer, output, termination))
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

/// Derive a [`ProcessTerminationReason`] from a runner result and
/// recorded shutdown state.
///
/// The recorded reason wins when set (a signal handler or the compose
/// orchestrator already classified the exit). Otherwise `Completed` for
/// clean returns and `RunnerError(msg)` for failures.
#[must_use]
pub fn derive_finalize_reason(
    runner_failure_message: Option<String>,
    termination: &TerminationRecorder,
) -> ProcessTerminationReason {
    if let Some(reason) = termination.reason_taken() {
        return reason;
    }
    if termination.intent().is_cancelled() {
        // The token fired without going through `cancel(reason)` — an
        // external clone of the token was cancelled directly. The runner
        // result still classifies the exit correctly, but surface the
        // unusual path.
        warn!("shutdown intent fired without a recorded reason; classifying from the runner result");
    }
    match runner_failure_message {
        None => ProcessTerminationReason::Completed,
        Some(msg) => ProcessTerminationReason::RunnerError(msg.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_status_table_matches_rev17_j1() {
        assert_eq!(
            terminal_status_for(&ProcessTerminationReason::Completed),
            ProcessStatus::Stopped
        );
        assert_eq!(
            terminal_status_for(&ProcessTerminationReason::RunnerError("boom".into())),
            ProcessStatus::Failed
        );
        assert_eq!(
            terminal_status_for(&ProcessTerminationReason::SignalTerm),
            ProcessStatus::Killed
        );
        assert_eq!(
            terminal_status_for(&ProcessTerminationReason::SignalInt),
            ProcessStatus::Killed
        );
    }

    #[test]
    fn new_recorder_starts_uncancelled_and_unrecorded() {
        let r = TerminationRecorder::new();
        assert!(!r.intent().is_cancelled());
        assert!(r.reason_taken().is_none());
    }

    #[test]
    fn cancel_records_reason_and_fires_intent() {
        let r = TerminationRecorder::new();
        r.cancel(ProcessTerminationReason::Completed);
        assert!(r.intent().is_cancelled());
        let reason = r.reason_taken().expect("reason recorded");
        assert!(matches!(reason, ProcessTerminationReason::Completed));
        // intent stays cancelled even after the slot is drained.
        assert!(r.intent().is_cancelled());
    }

    #[test]
    fn first_cancel_wins() {
        let r = TerminationRecorder::new();
        r.cancel(ProcessTerminationReason::SignalTerm);
        r.cancel(ProcessTerminationReason::SignalInt);
        let reason = r.reason_taken().expect("reason recorded");
        assert!(
            matches!(reason, ProcessTerminationReason::SignalTerm),
            "first reason should win, got {reason:?}"
        );
    }

    #[test]
    fn clones_share_reason_slot_and_intent() {
        let a = TerminationRecorder::new();
        let b = a.clone();
        b.cancel(ProcessTerminationReason::RunnerError("boom".into()));
        assert!(a.intent().is_cancelled());
        let reason = a.reason_taken().expect("clone shares slot");
        assert!(matches!(reason, ProcessTerminationReason::RunnerError(_)));
    }

    #[test]
    fn derive_prefers_recorded_reason_over_runner_result() {
        let r = TerminationRecorder::new();
        r.cancel(ProcessTerminationReason::SignalInt);
        let reason = derive_finalize_reason(Some("boom".into()), &r);
        assert!(matches!(reason, ProcessTerminationReason::SignalInt));
    }

    #[test]
    fn derive_classifies_runner_results_when_nothing_recorded() {
        let r = TerminationRecorder::new();
        assert!(matches!(
            derive_finalize_reason(None, &r),
            ProcessTerminationReason::Completed
        ));
        assert!(matches!(
            derive_finalize_reason(Some("boom".into()), &r),
            ProcessTerminationReason::RunnerError(_)
        ));
    }

    #[tokio::test]
    async fn install_signal_handlers_does_not_panic() {
        // We can't reliably synthesize SIGINT/SIGTERM in unit tests, so
        // this just confirms the install call succeeds and the spawned
        // task exits cleanly when the recorder is cancelled externally.
        let r = TerminationRecorder::new();
        r.install_signal_handlers().expect("install");
        r.cancel(ProcessTerminationReason::Completed);
        // Yield long enough for the spawned task to observe the cancel
        // and exit. Test passes if nothing deadlocks or panics.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(r.intent().is_cancelled());
    }
}
