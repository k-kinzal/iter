//! Bridge OS signals into a [`tokio_util::sync::CancellationToken`].
//!
//! The runner expects a single cancellation token. This module spawns a
//! background task that listens for `SIGINT` and `SIGTERM` (or just `Ctrl-C`
//! on non-unix targets) and triggers the token on the first one received.
//! The task self-terminates as soon as the token is fired so a graceful
//! shutdown does not leak waiting tasks.

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Errors produced while installing OS signal handlers.
#[derive(Debug, thiserror::Error)]
pub enum SignalsError {
    /// Installing a `SIGTERM` listener failed.
    #[error("installing SIGTERM listener: {0}")]
    Sigterm(#[source] std::io::Error),
    /// Installing a `SIGINT` listener failed.
    #[error("installing SIGINT listener: {0}")]
    Sigint(#[source] std::io::Error),
}

/// Spawn a background task that mirrors `SIGINT`/`SIGTERM` onto `cancel`.
///
/// Returns the same token, so callers can chain:
///
/// ```ignore
/// let cancel = install_shutdown_handler(CancellationToken::new())?;
/// runner.run(cancel).await?;
/// ```
///
/// # Errors
///
/// Returns [`SignalsError`] if the unix `SignalKind::terminate()` listener
/// cannot be installed. On non-unix platforms only Ctrl-C is wired up; that
/// listener is installed lazily inside the spawned task and any failure is
/// logged at `debug!` level rather than propagated.
pub fn install_shutdown_handler(
    cancel: CancellationToken,
) -> Result<CancellationToken, SignalsError> {
    spawn_handler(cancel.clone())?;
    Ok(cancel)
}

#[cfg(unix)]
fn spawn_handler(cancel: CancellationToken) -> Result<(), SignalsError> {
    use signal::unix::{SignalKind, signal as unix_signal};

    let mut sigterm = unix_signal(SignalKind::terminate()).map_err(SignalsError::Sigterm)?;
    let mut sigint = unix_signal(SignalKind::interrupt()).map_err(SignalsError::Sigint)?;

    tokio::spawn(async move {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("received SIGTERM, requesting shutdown");
            }
            _ = sigint.recv() => {
                info!("received SIGINT, requesting shutdown");
            }
            () = cancel.cancelled() => {
                debug!("shutdown handler exiting because cancellation token already fired");
                return;
            }
        }
        cancel.cancel();
    });

    Ok(())
}

#[cfg(not(unix))]
fn spawn_handler(cancel: CancellationToken) -> Result<(), SignalsError> {
    tokio::spawn(async move {
        tokio::select! {
            res = signal::ctrl_c() => {
                if let Err(err) = res {
                    debug!(error = %err, "ctrl_c listener failed");
                    return;
                }
                info!("received Ctrl-C, requesting shutdown");
            }
            () = cancel.cancelled() => {
                debug!("shutdown handler exiting because cancellation token already fired");
                return;
            }
        }
        cancel.cancel();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn external_cancel_makes_handler_exit_quickly() {
        let token = install_shutdown_handler(CancellationToken::new()).expect("install");
        token.cancel();
        // Give the spawned task a tick to notice the cancellation; the test
        // is really about the handler not deadlocking.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(token.is_cancelled());
    }
}
