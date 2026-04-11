//! `ShutdownController` — the single source of cancellation for the
//! Runner and observers.
//!
//! Per rev17 §A2/§J1, the Process runtime composes four orchestrator
//! pieces. `ShutdownController` is the one that owns:
//!
//! - the [`CancellationToken`] handed to the runner and any
//!   cancellation-aware downstream task;
//! - a one-shot record of *why* shutdown happened
//!   ([`ProcessTerminationReason`]) so `finalize` can write the right
//!   terminal status;
//! - an optional background task that mirrors `SIGINT`/`SIGTERM` (the
//!   responsibility moved here from `iter_compose::install_shutdown_handler`,
//!   which is removed in this refactor).
//!
//! The controller never writes to `~/.iter/proc/<id>/`. It only records
//! intent. The runtime reads [`ShutdownController::reason`] (or awaits
//! it via [`wait_for_reason`]) and then drives the status transition
//! through [`crate::process::status_file::ProcessStatusFile::transition`].
//!
//! [`wait_for_reason`]: ShutdownController::wait_for_reason

use std::error::Error;
use std::sync::{Arc, Mutex};

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Boxed error trait object carried inside
/// [`ProcessTerminationReason::RunnerError`].
///
/// Mirrors [`crate::runner::BoxError`] but is re-aliased here so the
/// `process` module doesn't have to depend on the runner's public
/// surface for a type that is fundamentally `std::error::Error`.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Why the process exited its main run loop.
///
/// Per rev17 §J1, this is the input the runtime needs in order to pick a
/// terminal [`ProcessStatus`](crate::process::ProcessStatus):
///
/// | reason            | terminal status |
/// |-------------------|-----------------|
/// | `Completed`       | `Stopped`       |
/// | `RunnerError(_)`  | `Failed`        |
/// | `SignalTerm`      | `Killed`        |
/// | `SignalInt`       | `Killed`        |
/// | `PanicCaught`     | `Failed`        |
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
    /// The runner task ended with `JoinError::is_panic() == true`.
    PanicCaught,
}

impl ProcessTerminationReason {
    /// `true` when this reason was triggered by an OS signal.
    #[must_use]
    pub fn is_signal(&self) -> bool {
        matches!(self, Self::SignalTerm | Self::SignalInt)
    }

    /// `true` when this reason represents a clean completion.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Single source of cancellation + termination-reason recording for a
/// Process Runtime.
///
/// Cloning a [`ShutdownController`] is cheap (`Arc` + `CancellationToken`
/// internals) and shares the same termination-reason slot. The intended
/// pattern is:
///
/// 1. construct one with [`Self::new`];
/// 2. optionally call [`Self::install_signal_handlers`] to mirror
///    SIGINT/SIGTERM onto the token;
/// 3. hand [`Self::token`] clones to the runner and observers;
/// 4. observe completion through [`Self::wait_for_reason`] (or peek with
///    [`Self::reason`]);
/// 5. let `finalize` translate the reason into a terminal status.
#[derive(Debug, Clone)]
pub struct ShutdownController {
    cancel: CancellationToken,
    reason: Arc<Mutex<Option<ProcessTerminationReason>>>,
}

impl ShutdownController {
    /// Create a fresh controller backed by a brand-new
    /// [`CancellationToken`] and an empty reason slot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            reason: Arc::new(Mutex::new(None)),
        }
    }

    /// Build a controller around a pre-existing [`CancellationToken`].
    ///
    /// Useful when the caller already owns a token (for example because
    /// it bridges another subsystem) and wants the controller to share
    /// it rather than introducing a second one.
    #[must_use]
    pub fn with_token(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            reason: Arc::new(Mutex::new(None)),
        }
    }

    /// Borrow the underlying [`CancellationToken`].
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Trigger shutdown and record the reason.
    ///
    /// First call wins: subsequent invocations leave the recorded reason
    /// untouched and only ensure the [`CancellationToken`] is fired
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
        self.cancel.cancel();
    }

    /// Peek the recorded reason without waiting.
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

    /// `true` iff the underlying [`CancellationToken`] has fired.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Wait for the controller to be cancelled and return the recorded
    /// reason.
    ///
    /// If cancellation happened without a reason being recorded (which
    /// only occurs when an external clone of the token was cancelled
    /// directly bypassing [`Self::cancel`]), the returned reason is
    /// `None`. The runtime treats that as `Completed` for accounting
    /// purposes but logs at `warn!` so unexpected paths surface.
    pub async fn wait_for_reason(&self) -> Option<ProcessTerminationReason> {
        self.cancel.cancelled().await;
        let taken = self.reason_taken();
        if taken.is_none() {
            warn!(
                "shutdown token cancelled without a recorded reason; \
                 falling back to None (see ShutdownController docs)"
            );
        }
        taken
    }

    /// Mirror `SIGINT`/`SIGTERM` onto this controller.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// Spawns a background task that records [`SignalTerm`] or
    /// [`SignalInt`] and triggers the [`CancellationToken`]. The task
    /// self-terminates if the token fires for any other reason.
    ///
    /// On non-unix targets, only `Ctrl-C` is wired and it records
    /// [`SignalInt`].
    ///
    /// Returns an error if the unix signal listeners cannot be installed.
    /// On non-unix targets the listener is installed lazily inside the
    /// spawned task and any failure is logged at `debug!` rather than
    /// propagated (matching the legacy `iter_compose::install_shutdown_handler`
    /// behaviour).
    ///
    /// [`SignalTerm`]: ProcessTerminationReason::SignalTerm
    /// [`SignalInt`]: ProcessTerminationReason::SignalInt
    pub fn install_signal_handlers(&self) -> std::io::Result<()> {
        spawn_handler(self.clone())
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
fn spawn_handler(controller: ShutdownController) -> std::io::Result<()> {
    use signal::unix::{SignalKind, signal as unix_signal};

    let mut sigterm = unix_signal(SignalKind::terminate())?;
    let mut sigint = unix_signal(SignalKind::interrupt())?;

    let cancel_clone = controller.cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("received SIGTERM, requesting shutdown");
                controller.cancel(ProcessTerminationReason::SignalTerm);
            }
            _ = sigint.recv() => {
                info!("received SIGINT, requesting shutdown");
                controller.cancel(ProcessTerminationReason::SignalInt);
            }
            () = cancel_clone.cancelled() => {
                debug!(
                    "shutdown handler exiting because the controller was cancelled \
                     externally"
                );
            }
        }
    });
    Ok(())
}

#[cfg(not(unix))]
fn spawn_handler(controller: ShutdownController) -> std::io::Result<()> {
    let cancel_clone = controller.cancel.clone();
    tokio::spawn(async move {
        tokio::select! {
            res = signal::ctrl_c() => {
                match res {
                    Ok(()) => {
                        info!("received Ctrl-C, requesting shutdown");
                        controller.cancel(ProcessTerminationReason::SignalInt);
                    }
                    Err(err) => {
                        debug!(error = %err, "ctrl_c listener failed");
                    }
                }
            }
            () = cancel_clone.cancelled() => {
                debug!(
                    "shutdown handler exiting because the controller was cancelled \
                     externally"
                );
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[test]
    fn new_controller_starts_uncancelled_and_unrecorded() {
        let c = ShutdownController::new();
        assert!(!c.is_cancelled());
        assert!(c.reason_taken().is_none());
    }

    #[test]
    fn cancel_records_reason_and_fires_token() {
        let c = ShutdownController::new();
        c.cancel(ProcessTerminationReason::Completed);
        assert!(c.is_cancelled());
        let r = c.reason_taken().expect("reason recorded");
        assert!(matches!(r, ProcessTerminationReason::Completed));
        // token still cancelled even after the slot is drained.
        assert!(c.is_cancelled());
    }

    #[test]
    fn first_cancel_wins() {
        let c = ShutdownController::new();
        c.cancel(ProcessTerminationReason::SignalTerm);
        c.cancel(ProcessTerminationReason::SignalInt);
        let r = c.reason_taken().expect("reason recorded");
        assert!(
            matches!(r, ProcessTerminationReason::SignalTerm),
            "first reason should win, got {r:?}"
        );
    }

    #[test]
    fn clones_share_reason_slot_and_token() {
        let a = ShutdownController::new();
        let b = a.clone();
        b.cancel(ProcessTerminationReason::PanicCaught);
        assert!(a.is_cancelled());
        let r = a.reason_taken().expect("clone shares slot");
        assert!(matches!(r, ProcessTerminationReason::PanicCaught));
    }

    #[test]
    fn helpers_classify_reasons() {
        assert!(ProcessTerminationReason::Completed.is_completed());
        assert!(!ProcessTerminationReason::Completed.is_signal());
        assert!(ProcessTerminationReason::SignalInt.is_signal());
        assert!(ProcessTerminationReason::SignalTerm.is_signal());
        assert!(!ProcessTerminationReason::PanicCaught.is_signal());
        assert!(!ProcessTerminationReason::PanicCaught.is_completed());
    }

    #[tokio::test]
    async fn wait_for_reason_returns_recorded_reason() {
        let c = ShutdownController::new();
        let waiter = {
            let c2 = c.clone();
            tokio::spawn(async move { c2.wait_for_reason().await })
        };
        // Give the waiter a moment to register, then cancel.
        tokio::time::sleep(Duration::from_millis(5)).await;
        c.cancel(ProcessTerminationReason::Completed);
        let observed = timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter")
            .expect("join")
            .expect("reason");
        assert!(matches!(observed, ProcessTerminationReason::Completed));
    }

    #[tokio::test]
    async fn external_token_cancellation_yields_none_reason() {
        let token = CancellationToken::new();
        let c = ShutdownController::with_token(token.clone());
        token.cancel();
        // No reason was recorded: wait_for_reason should resolve to None.
        let observed = timeout(Duration::from_millis(50), c.wait_for_reason())
            .await
            .expect("immediate resolution");
        assert!(observed.is_none());
    }

    #[tokio::test]
    async fn install_signal_handlers_does_not_panic() {
        // We can't reliably synthesize SIGINT/SIGTERM in unit tests, so
        // this just confirms the install call succeeds and the spawned
        // task exits cleanly when the controller is cancelled externally.
        let c = ShutdownController::new();
        c.install_signal_handlers().expect("install");
        c.cancel(ProcessTerminationReason::Completed);
        // Yield long enough for the spawned task to observe the cancel
        // and exit. Test passes if nothing deadlocks or panics.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(c.is_cancelled());
    }
}
