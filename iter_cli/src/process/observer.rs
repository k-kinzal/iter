//! `LifecycleObserver` — the process-runtime consumer that persists
//! [`RunnerLifecycleEvent`] events to tracing / `log.ndjson`.
//!
//! The observer traits ([`RunnerObserver`], [`DynRunnerObserver`]) are
//! defined by the runner module that owns the lifecycle contract. This
//! module re-exports them for convenience and houses the concrete
//! [`LifecycleObserver`] implementation that the process runtime plugs
//! into the runner's observer vector.
//!
//! ### Where lifecycle events go
//!
//! [`LifecycleObserver`] re-emits each lifecycle record as a
//! [`tracing::info!`] event under the `iter::lifecycle` target. The
//! tracing subscriber installed by the runtime fans those events into
//! `log.ndjson` (alongside agent stdio and ad-hoc runner tracing) via
//! [`crate::process::log::ProcessLogSink`]. The on-disk record is the
//! single docker-logs-parity NDJSON stream — there is no separate
//! `events.ndjson`.
//!
//! ### Bounded mpsc + writer task
//!
//! [`LifecycleObserver`] still wraps a dedicated tokio task connected
//! via a [`tokio::sync::mpsc`] channel. The capacity defaults to
//! [`DEFAULT_LIFECYCLE_BUFFER`] and can be overridden through the
//! `ITER_PROCESS_LIFECYCLE_BUFFER` environment variable. `observe()`
//! `await`s `Sender::send` so the runner is back-pressured if the writer
//! task falls behind. When the writer task has exited or the sender has
//! been dropped via [`LifecycleObserver::shutdown`], `observe()` returns
//! [`ObserverError::WriterStopped`] rather than silently dropping
//! events.
//!
//! ### Shutdown
//!
//! The Runtime calls [`LifecycleObserver::shutdown`] from `finalize`
//! (best-effort drain). Shutdown drops the sender, awaits the
//! writer task, and propagates any error.

use std::env;
use std::future::Future;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::process::error::ObserverError;
use crate::process::log::LogSender;
use iter_core::log::LogStream;
use iter_core::runner::BoxError;
use iter_core::runner::lifecycle::{RedactedMetadata, RunnerLifecycleEvent};
use iter_core::signal::SignalId;

pub(crate) use iter_core::runner::observer::{DynRunnerObserver, ObserveFuture, RunnerObserver};

/// Default in-flight capacity for the [`LifecycleObserver`] mpsc channel.
///
/// Override at runtime via the `ITER_PROCESS_LIFECYCLE_BUFFER`
/// environment variable. The runtime parses unsigned integers; any
/// parse failure or zero value falls back to this default.
pub(crate) const DEFAULT_LIFECYCLE_BUFFER: usize = 1024;

/// Environment variable name read by [`LifecycleObserver::open_in`] to
/// override [`DEFAULT_LIFECYCLE_BUFFER`].
pub(crate) const LIFECYCLE_BUFFER_ENV: &str = "ITER_PROCESS_LIFECYCLE_BUFFER";

/// Tracing target every lifecycle event is emitted under. Subscribers
/// wishing to filter the lifecycle stream alone can use this constant
/// in their `EnvFilter` or layered `Targets` configuration.
pub(crate) const LIFECYCLE_TARGET: &str = "iter::lifecycle";

/// Persisting observer that re-emits [`RunnerLifecycleEvent`] events as
/// `tracing::info!` records under [`LIFECYCLE_TARGET`].
///
/// The observer holds an `mpsc::Sender` whose receiver is owned by a
/// dedicated writer task. `observe()` awaits the send so the runner is
/// back-pressured when the writer is busy. The writer task drains the
/// receiver and emits one tracing event per record; the tracing
/// subscriber wired by the runtime then routes each event into
/// `log.ndjson`.
///
/// The sender is wrapped in `Mutex<Option<...>>` so `shutdown` can
/// `take` and drop it; that is what makes the writer task's `recv`
/// resolve to `None` and exit. (`Sender::closed` is the wrong primitive
/// here: it waits for the *receiver* to be dropped, but the receiver is
/// held by the writer task, which only exits once all senders are
/// dropped — so `closed().await` would deadlock.)
pub(crate) struct LifecycleObserver {
    sender: Mutex<Option<mpsc::Sender<RunnerLifecycleEvent>>>,
    writer: Mutex<Option<JoinHandle<Result<(), ObserverError>>>>,
}

impl LifecycleObserver {
    /// Spawn the writer task in `dir` and return the observer.
    ///
    /// `dir` is accepted for API compatibility with the prior
    /// file-backed observer; the new tracing-fan-out implementation
    /// does not open any file. `log_sender` is the back-pressured
    /// path into `log.ndjson`: when `Some`, the writer task pushes
    /// each lifecycle record directly through it (so a full
    /// [`ProcessLogSink`](crate::process::log::ProcessLogSink) channel
    /// blocks the writer rather than silently dropping the event).
    /// `None` keeps the observer running in tracing-only mode for
    /// foreground/test bootstraps that do not own a `log.ndjson`.
    /// Channel capacity is read from
    /// [`LIFECYCLE_BUFFER_ENV`]; non-positive or unparsable values fall
    /// back to [`DEFAULT_LIFECYCLE_BUFFER`].
    ///
    /// # Errors
    ///
    /// Currently never returns an error — the signature stays
    /// `Result<_, ObserverError>` so future hardening (e.g. validating
    /// `dir` is writable) can plug in without churn at every call
    /// site.
    pub(crate) async fn open_in(
        _dir: &std::path::Path,
        log_sender: Option<LogSender>,
    ) -> Result<Self, ObserverError> {
        std::future::ready(Ok(Self::with_capacity(read_capacity_env(), log_sender))).await
    }

    /// Build an observer with an explicit channel capacity. Internal —
    /// callers should use [`Self::open_in`].
    fn with_capacity(capacity: usize, log_sender: Option<LogSender>) -> Self {
        let (tx, rx) = mpsc::channel::<RunnerLifecycleEvent>(capacity);
        let handle = tokio::spawn(run_writer(rx, log_sender));
        Self {
            sender: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(handle)),
        }
    }

    /// Build an observer that drops every event into a no-op task.
    ///
    /// Useful in unit tests of the runner where the writer task isn't
    /// the subject under test.
    #[cfg(test)]
    #[doc(hidden)]
    #[must_use]
    pub(crate) fn null() -> Self {
        let (tx, mut rx) = mpsc::channel::<RunnerLifecycleEvent>(DEFAULT_LIFECYCLE_BUFFER);
        let handle = tokio::spawn(async move {
            while rx.recv().await.is_some() {}
            Ok(())
        });
        Self {
            sender: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(handle)),
        }
    }

    /// Stop the writer task.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// Takes and drops the sender so the writer's `recv` resolves to
    /// `None`, then awaits the writer task and surfaces its result.
    /// Idempotent: subsequent calls return `Ok(())`.
    pub(crate) async fn shutdown(&self) -> Result<(), ObserverError> {
        // Drop the sender first so the writer's recv() can resolve.
        {
            let mut slot = self.sender.lock().await;
            drop(slot.take());
        }
        // Then take the join handle (idempotent).
        let mut writer_slot = self.writer.lock().await;
        let Some(handle) = writer_slot.take() else {
            return Ok(());
        };
        match handle.await {
            Ok(res) => res,
            Err(_) => Err(ObserverError::WriterStopped),
        }
    }
}

impl LifecycleObserver {
    /// Direct enqueue for use by [`RunnerObserver::observe`]. Returns
    /// [`ObserverError::WriterStopped`] when the writer task is gone
    /// or the sender has been dropped via [`Self::shutdown`].
    async fn enqueue(&self, lifecycle: RunnerLifecycleEvent) -> Result<(), ObserverError> {
        let sender = {
            let slot = self.sender.lock().await;
            slot.as_ref().cloned().ok_or(ObserverError::WriterStopped)?
        };
        sender
            .send(lifecycle)
            .await
            .map_err(|_| ObserverError::WriterStopped)
    }
}

impl RunnerObserver for LifecycleObserver {
    fn observe<'a>(
        &'a self,
        lifecycle: &'a RunnerLifecycleEvent,
    ) -> impl Future<Output = Result<(), BoxError>> + Send + 'a {
        let cloned = lifecycle.clone();
        async move {
            self.enqueue(cloned)
                .await
                .map_err(|e| -> BoxError { Box::new(e) })?;
            Ok(())
        }
    }
}

fn read_capacity_env() -> usize {
    match env::var(LIFECYCLE_BUFFER_ENV) {
        Ok(s) => match s.parse::<usize>() {
            Ok(n) if n > 0 => n,
            _ => DEFAULT_LIFECYCLE_BUFFER,
        },
        Err(_) => DEFAULT_LIFECYCLE_BUFFER,
    }
}

/// Drain the lifecycle mpsc channel, re-emitting each record as a
/// tracing event and (when wired) pushing the same record into
/// `log.ndjson` via the back-pressured [`LogSender`] path. The
/// tracing subscriber's `LogJsonMakeWriter` is configured to filter out
/// `iter::lifecycle` so this is the *only* path lifecycle records take
/// into the NDJSON file — `LogSender::send_line` awaits, so a slow
/// disk back-pressures the lifecycle queue (and through it, the runner)
/// instead of silently losing post-mortem data. On sender error the
/// writer falls back to tracing-only mode for the rest of the run.
async fn run_writer(
    mut rx: mpsc::Receiver<RunnerLifecycleEvent>,
    log_sender: Option<LogSender>,
) -> Result<(), ObserverError> {
    let mut log_sender = log_sender;
    while let Some(ev) = rx.recv().await {
        emit_lifecycle(&ev);
        if let Some(sender) = &log_sender {
            let line = format_lifecycle_line(&ev);
            if let Err(e) = sender.send_line(LogStream::Stderr, line).await {
                tracing::warn!(
                    target: LIFECYCLE_TARGET,
                    error = %e,
                    "log.ndjson sender stopped; dropping further direct lifecycle writes"
                );
                // Stop trying — the writer task is gone, every further
                // send would fail the same way.
                log_sender = None;
            }
        }
    }
    Ok(())
}

/// Re-emit a [`RunnerLifecycleEvent`] record as a `tracing::info!` event under
/// [`LIFECYCLE_TARGET`].
///
/// Each variant maps to a single human-readable message line plus the
/// minimal set of structured fields needed to correlate with the rest
/// of `log.ndjson`. Field names are stable so downstream consumers can
/// `grep` or filter by structured field.
fn emit_lifecycle(ev: &RunnerLifecycleEvent) {
    match ev {
        RunnerLifecycleEvent::BootstrapStarted { started_at } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "bootstrap_started",
                started_at = %fmt_ts(started_at),
                "runner bootstrap started"
            );
        }
        RunnerLifecycleEvent::BootstrapFailed { error } => {
            tracing::error!(
                target: LIFECYCLE_TARGET,
                event = "bootstrap_failed",
                error = %error,
                "runner bootstrap failed"
            );
        }
        RunnerLifecycleEvent::SignalReceived {
            signal_id,
            metadata,
            ts,
        } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "signal_received",
                signal_id = %signal_id_short(*signal_id),
                metadata_keys = metadata_keys_count(metadata),
                ts = %fmt_ts(ts),
                "signal received"
            );
        }
        RunnerLifecycleEvent::WorkspaceSetup { signal_id, path } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "workspace_setup",
                signal_id = %signal_id_short(*signal_id),
                path = %path.display(),
                "workspace setup"
            );
        }
        RunnerLifecycleEvent::AgentStarting { signal_id } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "agent_starting",
                signal_id = %signal_id_short(*signal_id),
                "agent starting"
            );
        }
        RunnerLifecycleEvent::AgentFinished {
            signal_id,
            result,
            exit,
        } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "agent_finished",
                signal_id = %signal_id_short(*signal_id),
                result = %result,
                exit = ?exit,
                "agent finished"
            );
        }
        RunnerLifecycleEvent::WorkspaceTearDown { signal_id } => {
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "workspace_teardown",
                signal_id = %signal_id_short(*signal_id),
                "workspace teardown"
            );
        }
        RunnerLifecycleEvent::RunnerError {
            signal_id,
            error_source,
            error_message,
        } => {
            let signal_id_field = signal_id.map(signal_id_short);
            // The observability key is `source` (the error-source concept);
            // the structured JSON event keeps `"stage"` for backward
            // compatibility (R16). The two surfaces deliberately differ: the
            // human/tracing key follows the vocabulary, the wire key is pinned.
            tracing::error!(
                target: LIFECYCLE_TARGET,
                event = "runner_error",
                signal_id = ?signal_id_field,
                source = error_source.as_str(),
                error = %error_message,
                "runner error"
            );
        }
        RunnerLifecycleEvent::RunnerFinished {
            termination_reason,
            iteration_count,
            last_signal_id,
            event_handler_error_count,
            observer_error_count,
        } => {
            let last_signal_id_field = last_signal_id.map(signal_id_short);
            tracing::info!(
                target: LIFECYCLE_TARGET,
                event = "runner_finished",
                reason = ?termination_reason,
                iterations = *iteration_count,
                last_signal_id = ?last_signal_id_field,
                event_handler_error_count = *event_handler_error_count,
                observer_error_count = *observer_error_count,
                "runner finished"
            );
        }
    }
}

/// Format a [`RunnerLifecycleEvent`] as the single-line message text that
/// the writer task pushes into `log.ndjson` via
/// [`LogSender::send_line`]. The shape mirrors the tracing
/// subscriber's compact format ("`<message>` field=value field=value")
/// so the NDJSON file and the foreground stderr stream remain
/// human-comparable.
fn format_lifecycle_line(ev: &RunnerLifecycleEvent) -> String {
    match ev {
        RunnerLifecycleEvent::BootstrapStarted { started_at } => {
            format!(
                "runner bootstrap started event=bootstrap_started started_at={}",
                fmt_ts(started_at)
            )
        }
        RunnerLifecycleEvent::BootstrapFailed { error } => {
            format!("runner bootstrap failed event=bootstrap_failed error={error}")
        }
        RunnerLifecycleEvent::SignalReceived {
            signal_id,
            metadata,
            ts,
        } => {
            format!(
                "signal received event=signal_received signal_id={} metadata_keys={} ts={}",
                signal_id_short(*signal_id),
                metadata_keys_count(metadata),
                fmt_ts(ts),
            )
        }
        RunnerLifecycleEvent::WorkspaceSetup { signal_id, path } => {
            format!(
                "workspace setup event=workspace_setup signal_id={} path={}",
                signal_id_short(*signal_id),
                path.display()
            )
        }
        RunnerLifecycleEvent::AgentStarting { signal_id } => {
            format!(
                "agent starting event=agent_starting signal_id={}",
                signal_id_short(*signal_id)
            )
        }
        RunnerLifecycleEvent::AgentFinished {
            signal_id,
            result,
            exit,
        } => {
            format!(
                "agent finished event=agent_finished signal_id={} result={} exit={:?}",
                signal_id_short(*signal_id),
                result,
                exit
            )
        }
        RunnerLifecycleEvent::WorkspaceTearDown { signal_id } => {
            format!(
                "workspace teardown event=workspace_teardown signal_id={}",
                signal_id_short(*signal_id)
            )
        }
        RunnerLifecycleEvent::RunnerError {
            signal_id,
            error_source,
            error_message,
        } => {
            let signal_id_field = signal_id.map_or_else(|| "None".to_owned(), signal_id_short);
            let source = error_source.as_str();
            format!(
                "runner error event=runner_error signal_id={signal_id_field} source={source} error={error_message}"
            )
        }
        RunnerLifecycleEvent::RunnerFinished {
            termination_reason,
            iteration_count,
            last_signal_id,
            event_handler_error_count,
            observer_error_count,
        } => {
            let last_signal_id_field =
                last_signal_id.map_or_else(|| "None".to_owned(), signal_id_short);
            format!(
                "runner finished event=runner_finished reason={termination_reason:?} iterations={iteration_count} last_signal_id={last_signal_id_field} event_handler_error_count={event_handler_error_count} observer_error_count={observer_error_count}"
            )
        }
    }
}

fn fmt_ts(ts: &chrono::DateTime<chrono::Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn signal_id_short(id: SignalId) -> String {
    let s = id.to_string();
    if s.len() > 16 { s[..16].to_owned() } else { s }
}

fn metadata_keys_count(meta: &RedactedMetadata) -> usize {
    meta.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample_event(signal_id: SignalId) -> RunnerLifecycleEvent {
        RunnerLifecycleEvent::AgentFinished {
            signal_id,
            result: "success".to_owned(),
            exit: Some(0),
        }
    }

    #[tokio::test]
    async fn observe_then_shutdown_drains_writer_cleanly() {
        let observer = LifecycleObserver::open_in(std::path::Path::new("/tmp"), None)
            .await
            .expect("open");
        let id = SignalId::new();
        for _ in 0..5 {
            RunnerObserver::observe(&observer, &sample_event(id))
                .await
                .expect("observe");
        }
        observer.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let observer = LifecycleObserver::open_in(std::path::Path::new("/tmp"), None)
            .await
            .expect("open");
        observer.shutdown().await.expect("first");
        observer.shutdown().await.expect("second is no-op");
    }

    #[tokio::test]
    async fn observe_after_shutdown_returns_writer_stopped() {
        let observer = LifecycleObserver::open_in(std::path::Path::new("/tmp"), None)
            .await
            .expect("open");
        observer.shutdown().await.expect("shutdown");
        let id = SignalId::new();
        let err = RunnerObserver::observe(&observer, &sample_event(id))
            .await
            .expect_err("err");
        let down: &ObserverError = err
            .downcast_ref::<ObserverError>()
            .expect("typed observer err");
        assert!(matches!(down, ObserverError::WriterStopped));
    }

    #[tokio::test]
    async fn null_observer_drops_events() {
        let observer = LifecycleObserver::null();
        let id = SignalId::new();
        for _ in 0..10 {
            RunnerObserver::observe(&observer, &sample_event(id))
                .await
                .expect("observe");
        }
        observer.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn capacity_env_override_takes_effect() {
        // SAFETY: this test mutates a process-global env var and restores it
        // before return; no other test in this module uses this key.
        unsafe { env::set_var(LIFECYCLE_BUFFER_ENV, "8") };
        assert_eq!(read_capacity_env(), 8);
        // SAFETY: same isolated test scope; remove the temporary override
        // before checking the default.
        unsafe { env::remove_var(LIFECYCLE_BUFFER_ENV) };
        assert_eq!(read_capacity_env(), DEFAULT_LIFECYCLE_BUFFER);
        // SAFETY: same isolated test scope; set an invalid value only for the
        // duration of this assertion.
        unsafe { env::set_var(LIFECYCLE_BUFFER_ENV, "0") };
        assert_eq!(read_capacity_env(), DEFAULT_LIFECYCLE_BUFFER);
        // SAFETY: same isolated test scope; set an invalid value only for the
        // duration of this assertion.
        unsafe { env::set_var(LIFECYCLE_BUFFER_ENV, "not-a-number") };
        assert_eq!(read_capacity_env(), DEFAULT_LIFECYCLE_BUFFER);
        // SAFETY: same isolated test scope; restore the process environment.
        unsafe {
            env::remove_var(LIFECYCLE_BUFFER_ENV);
        };
    }

    #[tokio::test]
    async fn dyn_observer_dispatch_works() {
        use iter_core::runner::DynRunnerObserver;
        use std::sync::Arc;

        let observer: Arc<dyn DynRunnerObserver> = Arc::new(
            LifecycleObserver::open_in(std::path::Path::new("/tmp"), None)
                .await
                .expect("open"),
        );
        let id = SignalId::new();
        DynRunnerObserver::observe(observer.as_ref(), &sample_event(id))
            .await
            .expect("observe");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[test]
    fn agent_finished_line_uses_result_field() {
        let line = format_lifecycle_line(&RunnerLifecycleEvent::AgentFinished {
            signal_id: SignalId::new(),
            result: "token_limit".to_owned(),
            exit: None,
        });
        assert!(line.contains("result=token_limit"), "got {line:?}");
    }
}
