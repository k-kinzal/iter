//! `ProcessRuntime` — thin orchestrator that composes the four pieces a
//! Process needs to run:
//!
//! 1. [`ProcessSession`] — owns the proc directory and the
//!    `Arc<ProcessStatusFile>` that every status writer routes through.
//! 2. [`ShutdownIntent`] — the shared cancellation token handed to the
//!    runner and any cancellation-aware downstream task. Classifying *why*
//!    the run ended (`ProcessTerminationReason`) is the run record's
//!    concern and lives with its operator (`iter_cli`'s
//!    `process_lifecycle`), which derives the terminal status that
//!    `finalize` writes.
//! 3. [`LifecycleObserver`] — re-emits [`RunnerLifecycleEvent`] events as
//!    `tracing::info!` records under
//!    [`iter::lifecycle`](crate::process::observer::LIFECYCLE_TARGET);
//!    the runtime's tracing subscriber routes them into `log.ndjson`
//!    alongside agent stdio.
//! 4. An [`OutputSink`] + optional [`LogSender`] — the active sink that
//!    captures agent output into the unified `log.ndjson` stream (or
//!    drops bytes when the policy is `Passthrough`).
//!
//! Per rev17 §A2 this orchestrator is intentionally thin. The `run` body
//! that ties it to a [`Runner`](iter_core::runner::Runner) lives in Phase F
//! (Runner needs to grow a `RunnerObserver` registration first). What
//! Phase E lands here is the *compose-and-finalize* surface: build a
//! runtime around the four components, hand out their handles, and
//! `finalize` correctly when the run loop returns.
//!
//! # `try_into_running` is gone
//!
//! Per rev10/§C4 the only paths that move a record to `Running` are the
//! locked startup writers (`ProcessStatusFile::locked_initial_write` for
//! foreground, `locked_adoption_write` for detached). The generic
//! `transition` API rejects every `to == Running` (`is_allowed` table).
//! The runtime therefore never tries to publish `Running` itself; it
//! only owns the *terminal* transition in `finalize`.
//!
//! # `finalize` is best-effort
//!
//! Per §B6, `finalize` always attempts the status transition even when
//! stdio drain or observer flush fails. Drain failures are logged in place;
//! the returned error is reserved for the only outcome callers act on: the
//! record was left non-terminal.
//!
//! [`OutputSink`]: iter_core::log::OutputSink

use std::sync::Arc;

use tracing::{info, warn};

use crate::process::error::ProcessError;
use crate::process::id::ProcessId;
use crate::process::interrupt::ShutdownIntent;
use crate::process::log::{LogSender, ProcessOutput};
use crate::process::observer::LifecycleObserver;
use crate::process::session::ProcessSession;
use crate::process::status::{ProcessStatus, StatusTransition};
use iter_core::log::OutputSink;

/// The four orchestrator pieces composed into one struct.
///
/// Constructed once at startup, after the locked startup writer has flipped
/// the record to `Running`. The Runner pulls cancellation / sink / observer
/// references out via the accessors and runs its loop. When the loop exits,
/// the runtime is consumed by [`Self::finalize`] which writes the terminal
/// status and logs any best-effort drain errors.
pub(crate) struct ProcessRuntime {
    session: Arc<ProcessSession>,
    shutdown: ShutdownIntent,
    observer: Arc<LifecycleObserver>,
    sink: Arc<dyn OutputSink>,
    log_sender: Option<LogSender>,
}

impl ProcessRuntime {
    /// Compose the four pieces into a runtime.
    ///
    /// All ownership is moved in; clones of the cancellation token,
    /// observer Arc, and output sink are obtained through the accessors.
    pub(crate) fn new(
        session: Arc<ProcessSession>,
        shutdown: ShutdownIntent,
        observer: Arc<LifecycleObserver>,
        output: ProcessOutput,
    ) -> Self {
        let (sink, log_sender) = output.into_parts();
        Self {
            session,
            shutdown,
            observer,
            sink,
            log_sender,
        }
    }

    /// Borrow the session — the proc directory + status file owner.
    #[must_use]
    pub(crate) fn session(&self) -> &Arc<ProcessSession> {
        &self.session
    }

    /// Process id this runtime is running.
    #[must_use]
    pub(crate) fn id(&self) -> ProcessId {
        self.session.id()
    }

    /// Borrow the [`ShutdownIntent`]. Clone it (cheap) to hand
    /// cancellation tokens to long-lived tasks.
    #[must_use]
    pub(crate) fn shutdown(&self) -> &ShutdownIntent {
        &self.shutdown
    }

    /// Borrow the [`LifecycleObserver`]. Clone the Arc to register it
    /// with a [`RunnerObserver`](iter_core::runner::RunnerObserver)-aware
    /// runner.
    #[must_use]
    pub(crate) fn observer(&self) -> &Arc<LifecycleObserver> {
        &self.observer
    }

    /// Clone the [`Arc<dyn OutputSink>`] for distribution to agents.
    #[must_use]
    pub(crate) fn sink(&self) -> Arc<dyn OutputSink> {
        self.sink.clone()
    }

    /// Clone the [`LogSender`] when one exists. `None` for
    /// `Passthrough` policy.
    #[must_use]
    pub(crate) fn log_sender(&self) -> Option<LogSender> {
        self.log_sender.clone()
    }

    /// Drain the sink + observer, then write the terminal status.
    ///
    /// Always attempts the status transition, even when the stdio flush or
    /// observer shutdown fail. Best-effort drain failures are logged in
    /// place and do not affect the return value.
    ///
    /// `target` is the terminal status the caller derived from the run's
    /// termination classification (`ProcessTerminationReason`, owned by
    /// `iter_cli`'s `process_lifecycle` per the rev17 §J1 table). The
    /// runtime only owns the *transition* to it, including the
    /// Initializing-fallback demotions in [`write_terminal`].
    pub(crate) async fn finalize(self, target: ProcessStatus) -> Result<(), ProcessError> {
        let Self {
            session,
            shutdown: _shutdown,
            observer,
            sink,
            log_sender: _log_sender,
        } = self;

        // 1. Stop the lifecycle observer first. Its writer task emits
        //    `tracing::info!` events, which the active subscriber
        //    funnels into the same NDJSON pipeline as agent stdio.
        //    Draining the lifecycle queue *before* the stdio sink
        //    flushes guarantees those final lifecycle records reach
        //    `log.ndjson` ahead of the on-disk flush barrier.
        if let Err(e) = observer.shutdown().await {
            let err = observer_error_to_process_error(e);
            warn!(error = %err, "finalize observer drain error");
        }

        // 2. Flush any pending stdio bytes through the active sink and
        //    block on a writer-task drain barrier so every entry
        //    enqueued so far reaches disk.
        if let Err(e) = sink.flush().await {
            warn!(error = %e, "finalize stdio drain error");
        }
        // Drop the sink Arc. Pump tasks are owned by the runner via
        // JoinHandle and must already be terminated by the time
        // finalize is called.
        drop(sink);

        // 3. Write the caller-derived terminal status. Always attempt
        //    this — operator visibility into record state is what
        //    justifies the whole subsystem.
        match write_terminal(&session, target).await {
            Ok(transition) => {
                info!(
                    from = %transition.from,
                    to = %transition.to,
                    "finalize wrote terminal status"
                );
                Ok(())
            }
            Err(err) if record_is_already_terminal(&err) => {
                warn!(
                    error = %err,
                    "finalize observed an already-terminal status; treating as complete"
                );
                Ok(())
            }
            Err(err) => {
                warn!(error = %err, "finalize failed to write terminal status");
                Err(err)
            }
        }
    }
}

fn record_is_already_terminal(err: &ProcessError) -> bool {
    matches!(
        err,
        ProcessError::IllegalTransition {
            observed: Some(observed),
            ..
        } if observed.is_terminal()
    )
}

async fn write_terminal(
    session: &Arc<ProcessSession>,
    target: ProcessStatus,
) -> Result<StatusTransition, ProcessError> {
    let sf = session.status_file();
    // Try Running → target first (the expected case after a successful
    // `locked_initial_write` / `locked_adoption_write`). If the record was
    // never advanced past Initializing — e.g. because finalize was reached
    // before the locked startup writer ran — fall back to
    // Initializing → target so the runtime never leaves a stale
    // Initializing record behind.
    //
    // The transition table forbids `Initializing → Stopped` (a record that
    // never reached `Running` cannot have "stopped cleanly"). Demote the
    // target to `Failed` in that case so the fallback path stays inside the
    // allowed edges.
    match sf.clone().transition(ProcessStatus::Running, target).await {
        Ok(t) => Ok(t),
        Err(ProcessError::IllegalTransition {
            observed: Some(ProcessStatus::Initializing),
            ..
        }) => {
            let init_target = if target == ProcessStatus::Stopped {
                ProcessStatus::Failed
            } else {
                target
            };
            sf.transition(ProcessStatus::Initializing, init_target)
                .await
        }
        Err(ProcessError::IllegalTransition {
            observed: Some(observed),
            ..
        }) if observed.is_terminal() => {
            // Already terminal. Don't overwrite — log and report.
            warn!(
                ?observed,
                ?target,
                "finalize observed an already-terminal status; not overwriting"
            );
            Err(ProcessError::IllegalTransition {
                from: ProcessStatus::Running,
                to: target,
                observed: Some(observed),
            })
        }
        Err(e) => Err(e),
    }
}

fn observer_error_to_process_error(e: crate::process::error::ObserverError) -> ProcessError {
    use crate::process::error::ObserverError;
    match e {
        ObserverError::Io(io) => ProcessError::Io(io),
        ObserverError::WriterStopped => ProcessError::Io(std::io::Error::other(
            "lifecycle observer writer task stopped before finalize",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::id::Pid;
    use crate::process::log::ProcessOutput;
    use crate::process::pid_file::ProcessIdentity;
    use crate::process::proc_info::ProcessStartTime;
    use crate::process::registry::{MetadataDraft, ProcessRegistry};
    use crate::process::status_file::ProcessStatusFile;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fake_identity() -> ProcessIdentity {
        if cfg!(target_os = "linux") {
            ProcessIdentity {
                pid: Pid::new(7777),
                start_time: ProcessStartTime::LinuxClockTicks(42),
                linux_boot_id: Some("0123456789abcdef-deadbeef".into()),
            }
        } else {
            ProcessIdentity {
                pid: Pid::new(7777),
                start_time: ProcessStartTime::MacosEpochMicros(1_700_000_000_000_000),
                linux_boot_id: None,
            }
        }
    }

    fn sample_draft() -> MetadataDraft {
        MetadataDraft {
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into()],
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        }
    }

    async fn build_runtime(tmp: &TempDir) -> ProcessRuntime {
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, lock) = registry
            .register_foreground("alpha", sample_draft())
            .await
            .expect("register");
        // Leak the LockGuard — the test only inspects status file state.
        std::mem::forget(lock);

        let observer = Arc::new(
            LifecycleObserver::open_in(session.paths().dir(), None)
                .await
                .expect("observer"),
        );
        ProcessRuntime::new(
            session,
            ShutdownIntent::new(),
            observer,
            ProcessOutput::noop(),
        )
    }

    async fn read_status(tmp: &TempDir, id: ProcessId) -> ProcessStatus {
        let paths = crate::process::paths::ProcPaths::open_existing(tmp.path(), id).expect("paths");
        let sf = ProcessStatusFile::open_for_existing(paths)
            .await
            .expect("open status");
        sf.read_status().await.expect("status")
    }

    #[tokio::test]
    async fn finalize_writes_killed_from_initializing_when_record_never_ran() {
        // The locked startup writer was never called, so the record is
        // still Initializing. finalize must still write a terminal
        // status (the Initializing → Killed fallback).
        let tmp = TempDir::new().unwrap();
        let runtime = build_runtime(&tmp).await;
        let id = runtime.id();
        runtime
            .finalize(ProcessStatus::Killed)
            .await
            .expect("finalize");

        // Cross-process verify by re-opening the status file.
        let observed = read_status(&tmp, id).await;
        assert_eq!(observed, ProcessStatus::Killed);
    }

    #[tokio::test]
    async fn finalize_writes_failed_from_initializing_for_runner_error() {
        let tmp = TempDir::new().unwrap();
        let runtime = build_runtime(&tmp).await;
        let id = runtime.id();
        runtime
            .finalize(ProcessStatus::Failed)
            .await
            .expect("finalize");
        assert_eq!(read_status(&tmp, id).await, ProcessStatus::Failed);
    }

    #[tokio::test]
    async fn finalize_writes_stopped_after_running() {
        // Production happy-path: record advanced to Running by
        // `locked_initial_write`, then finalized with the clean-exit
        // target the operator derived (Stopped).
        let tmp = TempDir::new().unwrap();
        let runtime = build_runtime(&tmp).await;
        runtime
            .session()
            .status_file()
            .clone()
            .locked_initial_write(fake_identity(), runtime.session().paths().clone())
            .await
            .expect("locked_initial_write");
        let id = runtime.id();
        runtime
            .finalize(ProcessStatus::Stopped)
            .await
            .expect("finalize");
        assert_eq!(read_status(&tmp, id).await, ProcessStatus::Stopped);
    }

    #[tokio::test]
    async fn finalize_demotes_initializing_stopped_to_failed() {
        // If finalize is reached before the locked startup writer ever
        // ran, the record is still Initializing. The transition table
        // forbids Initializing → Stopped, so the runtime demotes the
        // target to Failed ("never started" cannot have "stopped
        // cleanly"). Killed and Failed targets already satisfy the
        // table.
        let tmp = TempDir::new().unwrap();
        let runtime = build_runtime(&tmp).await;
        let id = runtime.id();
        runtime
            .finalize(ProcessStatus::Stopped)
            .await
            .expect("finalize");
        assert_eq!(read_status(&tmp, id).await, ProcessStatus::Failed);
    }

    #[tokio::test]
    async fn finalize_succeeds_when_status_already_terminal() {
        // First finalize writes Killed. Second finalize observes a
        // terminal record. That is already the desired record property, so
        // finalize treats it as complete.
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open");
        let (session, lock) = registry
            .register_foreground("beta", sample_draft())
            .await
            .expect("register");
        std::mem::forget(lock);
        let observer = Arc::new(
            LifecycleObserver::open_in(session.paths().dir(), None)
                .await
                .expect("observer"),
        );
        let runtime = ProcessRuntime::new(
            session.clone(),
            ShutdownIntent::new(),
            observer,
            ProcessOutput::noop(),
        );
        let id = session.id();
        runtime
            .finalize(ProcessStatus::Killed)
            .await
            .expect("first finalize");
        assert_eq!(read_status(&tmp, id).await, ProcessStatus::Killed);

        // Second finalize on the same record.
        let observer2 = Arc::new(
            LifecycleObserver::open_in(session.paths().dir(), None)
                .await
                .expect("observer"),
        );
        let runtime2 = ProcessRuntime::new(
            session,
            ShutdownIntent::new(),
            observer2,
            ProcessOutput::noop(),
        );
        runtime2
            .finalize(ProcessStatus::Stopped)
            .await
            .expect("already terminal is complete");
        assert_eq!(read_status(&tmp, id).await, ProcessStatus::Killed);
    }
}
