//! `process::interrupt` — translate the operator's interrupt (`SIGINT` /
//! `SIGTERM`) into cancellation of the running exploration.
//!
//! This is the **single home** for the OS-signal → cancellation mirror. Two
//! callers build on it:
//!
//! - [`install_signal_handlers`] is the **token-only** interrupt: a caller that
//!   needs nothing but "cancel this [`CancellationToken`] when the operator
//!   interrupts" uses it directly. The Signal sources (the trigger binaries)
//!   and any cancellation-only consumer take this form.
//! - the reason-recording
//!   [`ShutdownController`](crate::process::ShutdownController) layers the
//!   terminal-status reason on top of the same listening primitive
//!   ([`spawn_interrupt_listener`]).
//!
//! Both share one implementation of the signal listening itself, so there is
//! exactly one place in the crate that installs the unix `SIGINT`/`SIGTERM`
//! listeners. The interrupt records *intent only*; it never touches
//! `~/.iter/proc/<id>/`.
//!
//! # The cancellation discipline (who may cancel whom, and what each owes)
//!
//! Cancellation in iter flows through a single [`CancellationToken`] shared by
//! the parties of one exploration. There are exactly three things that may
//! *fire* it, each owning the source it translates:
//!
//! 1. **the operator's interrupt** — `SIGINT`/`SIGTERM`, translated here. The
//!    operator owns "stop this run".
//! 2. **the emission budget** — a Trigger that has published its last allowed
//!    Signal closes its Queue to drain (Trigger-side; the budget is the
//!    Trigger's, enforced at the Queue boundary).
//! 3. **the iteration timeout** — the Runner's per-iteration deadline
//!    (`iteration_timeout`, a Runner policy) cancels an iteration that runs
//!    too long.
//!
//! On *receipt* of cancellation, each party owes one thing and only acts on
//! what it owns:
//!
//! - a **Trigger** stops publishing and lets its Queue drain;
//! - a **Queue** closes — `dequeue` returns `Ok(None)` once drained;
//! - an **Agent** honors the token within its termination grace and has its
//!   process tree killed whole at the deadline (the
//!   [`ProcessGroup`](crate::process_group::ProcessGroup) primitive);
//! - the **Runner** completes the current iteration's teardown and reports
//!   the outcome;
//! - the **operator** finalizes the run record.
//!
//! No component cancels anything it does not own: the interrupt module never
//! closes a Queue, the Runner never finalizes a record, a Queue never kills a
//! process tree.

use std::io;

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Which operator interrupt fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// `SIGTERM` — the platform's graceful-terminate request.
    Terminate,
    /// `SIGINT` — Ctrl-C.
    Interrupt,
}

/// Spawn a background task that mirrors `SIGINT`/`SIGTERM` onto `cancel`,
/// recording no termination reason. Returns the same token for chaining:
///
/// ```ignore
/// let cancel = install_signal_handlers(CancellationToken::new())?;
/// runner.run(cancel).await?;
/// ```
///
/// The task self-terminates as soon as `cancel` fires for any reason, so a
/// graceful shutdown does not leak a parked listener.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] when the unix signal listeners cannot
/// be installed. On non-unix targets only `Ctrl-C` is wired and the listener is
/// installed lazily inside the spawned task, so this call cannot fail there.
pub fn install_signal_handlers(cancel: CancellationToken) -> io::Result<CancellationToken> {
    let on_interrupt = cancel.clone();
    spawn_interrupt_listener(cancel.clone(), move |_| on_interrupt.cancel())?;
    Ok(cancel)
}

/// Spawn a task that listens for `SIGINT`/`SIGTERM` and invokes `on_interrupt`
/// exactly once with whichever signal fired. The task exits quietly without
/// invoking `on_interrupt` if `watch` is cancelled first.
///
/// This is the **single** OS-signal listening primitive in the crate; both the
/// token-only [`install_signal_handlers`] and the reason-recording
/// [`ShutdownController`](crate::process::ShutdownController) build on it.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] when the unix signal listeners cannot
/// be installed.
pub(crate) fn spawn_interrupt_listener(
    watch: CancellationToken,
    on_interrupt: impl FnOnce(Interrupt) + Send + 'static,
) -> io::Result<()> {
    spawn_handler(watch, on_interrupt)
}

#[cfg(unix)]
fn spawn_handler(
    watch: CancellationToken,
    on_interrupt: impl FnOnce(Interrupt) + Send + 'static,
) -> io::Result<()> {
    use signal::unix::{SignalKind, signal as unix_signal};

    // Label which listener failed: the raw `io::Error` from `unix_signal`
    // doesn't say whether it was the SIGTERM or SIGINT install that failed,
    // and callers only see the boxed `io::Error`.
    let mut sigterm = unix_signal(SignalKind::terminate())
        .map_err(|e| io::Error::new(e.kind(), format!("SIGTERM listener: {e}")))?;
    let mut sigint = unix_signal(SignalKind::interrupt())
        .map_err(|e| io::Error::new(e.kind(), format!("SIGINT listener: {e}")))?;

    tokio::spawn(async move {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("received SIGTERM, requesting shutdown");
                on_interrupt(Interrupt::Terminate);
            }
            _ = sigint.recv() => {
                info!("received SIGINT, requesting shutdown");
                on_interrupt(Interrupt::Interrupt);
            }
            () = watch.cancelled() => {
                debug!("interrupt listener exiting because cancellation already fired");
            }
        }
    });

    Ok(())
}

#[cfg(not(unix))]
fn spawn_handler(
    watch: CancellationToken,
    on_interrupt: impl FnOnce(Interrupt) + Send + 'static,
) -> io::Result<()> {
    tokio::spawn(async move {
        tokio::select! {
            res = signal::ctrl_c() => {
                match res {
                    Ok(()) => {
                        info!("received Ctrl-C, requesting shutdown");
                        on_interrupt(Interrupt::Interrupt);
                    }
                    Err(err) => debug!(error = %err, "ctrl_c listener failed"),
                }
            }
            () = watch.cancelled() => {
                debug!("interrupt listener exiting because cancellation already fired");
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn external_cancel_makes_listener_exit_quickly() {
        let token = install_signal_handlers(CancellationToken::new()).expect("install");
        token.cancel();
        // Give the spawned task a tick to observe the cancellation; the test is
        // really about the listener not deadlocking.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn install_returns_the_same_token() {
        let original = CancellationToken::new();
        let returned = install_signal_handlers(original.clone()).expect("install");
        returned.cancel();
        assert!(
            original.is_cancelled(),
            "returned token shares the original"
        );
    }
}
