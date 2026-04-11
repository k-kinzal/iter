//! Bridge OS signals into a [`tokio_util::sync::CancellationToken`].

use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Errors produced while installing OS signal handlers.
#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    /// Installing a `SIGTERM` listener failed.
    #[error("installing SIGTERM listener: {0}")]
    Sigterm(#[source] std::io::Error),
    /// Installing a `SIGINT` listener failed.
    #[error("installing SIGINT listener: {0}")]
    Sigint(#[source] std::io::Error),
}

/// Spawn a background task that mirrors `SIGINT`/`SIGTERM` onto `cancel`.
///
/// Returns the same token for chaining.
///
/// # Errors
///
/// Returns [`ShutdownError`] if the signal listener cannot be installed.
pub fn install_shutdown_handler(
    cancel: CancellationToken,
) -> Result<CancellationToken, ShutdownError> {
    spawn_handler(cancel.clone())?;
    Ok(cancel)
}

#[cfg(unix)]
fn spawn_handler(cancel: CancellationToken) -> Result<(), ShutdownError> {
    use signal::unix::{SignalKind, signal as unix_signal};

    let mut sigterm = unix_signal(SignalKind::terminate()).map_err(ShutdownError::Sigterm)?;
    let mut sigint = unix_signal(SignalKind::interrupt()).map_err(ShutdownError::Sigint)?;

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
fn spawn_handler(cancel: CancellationToken) -> Result<(), ShutdownError> {
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
