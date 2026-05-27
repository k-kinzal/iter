//! Parent-side attach for `iter run` (no `--detach`).
//!
//! Foreground and detached runs both fork through
//! [`iter_core::process::spawn_detached`]: the difference is what the
//! parent does *after* the child is forked.
//!
//! - `--detach`: parent prints the new ULID and returns.
//! - no flag (this module): parent stays in the foreground, streams the
//!   child's captured stdout/stderr to its own fd 1/2, forwards
//!   SIGINT/SIGTERM to the child via [`ProcessHandle::stop`], and waits
//!   for the on-disk `<dir>/status` to flip terminal.
//!
//! From the user's perspective the visible stdio and Ctrl-C semantics
//! match the previous in-process run path; from the registry's
//! perspective the record is identical to a detached one (`iter ps` /
//! `iter logs` / `iter stop` all work the same way regardless of whether
//! the user passed `--detach`).

use std::sync::Arc;
use std::time::Duration;

use iter_core::log::{LogStream, NdjsonReader};
use iter_core::process::{
    ProcessError, ProcessHandle, ProcessId, ProcessRegistry, ProcessStatus,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::output::{IntoExitCode, exit_codes};

/// How often the parent re-reads `<dir>/status`. The status file is the
/// authoritative termination signal because the parent `mem::forget`s
/// the `Child` (no `wait()` is possible). 150 ms keeps Ctrl-C latency
/// imperceptible without thrashing the lock.
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, Error)]
pub enum AttachError {
    #[error("opening process registry: {0}")]
    OpenRegistry(#[source] ProcessError),
    #[error("opening process handle: {0}")]
    OpenHandle(#[source] ProcessError),
    #[error("opening log stream: {0}")]
    OpenLog(#[source] ProcessError),
    #[error("refreshing status: {0}")]
    Status(#[source] ProcessError),
    #[error("forwarding stop to child: {0}")]
    Stop(#[source] ProcessError),
}

impl IntoExitCode for AttachError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::OpenRegistry(_) => exit_codes::INTERNAL,
            Self::OpenHandle(_) | Self::OpenLog(_) | Self::Status(_) | Self::Stop(_) => {
                exit_codes::RUNTIME
            }
        }
    }
}

/// Attach to a freshly-spawned child and stream its captured stdio to
/// the parent's terminal until the child reaches a terminal status.
///
/// Returns the final [`ProcessStatus`] so the dispatcher can map it to
/// an exit code (`Stopped` → 0; `Failed`/`Killed` → non-zero).
///
/// # Errors
///
/// Returns [`AttachError`] when the registry / record / log files cannot
/// be opened, the status read fails, or signal forwarding fails.
pub async fn attach(id: ProcessId) -> Result<ProcessStatus, AttachError> {
    let registry = ProcessRegistry::open_default().map_err(AttachError::OpenRegistry)?;
    let handle = ProcessHandle::open(registry.proc_root(), id)
        .await
        .map_err(AttachError::OpenHandle)?;
    let record = handle.record().clone();

    let stop_notify = Arc::new(Notify::new());
    install_signal_forwarder(stop_notify.clone());

    let reader = record
        .tail_log_ndjson(true, None)
        .map_err(AttachError::OpenLog)?;
    let pump_task = tokio::spawn(pump_log_ndjson(reader));

    let mut stop_sent = false;
    let status = loop {
        tokio::select! {
            biased;
            () = stop_notify.notified(), if !stop_sent => {
                // First Ctrl-C / SIGTERM: ask the child to wind down.
                // Subsequent signals are ignored at this layer; the
                // operator can escalate via `iter kill <id>` from
                // another terminal.
                handle.stop().await.map_err(AttachError::Stop)?;
                stop_sent = true;
            }
            () = tokio::time::sleep(STATUS_POLL_INTERVAL) => {
                let s = handle
                    .refresh_status()
                    .await
                    .map_err(AttachError::Status)?;
                if s.is_terminal() {
                    break s;
                }
            }
        }
    };

    pump_task.abort();
    drop(pump_task.await);

    Ok(status)
}

/// Map a terminal [`ProcessStatus`] to an exit code.
///
/// `Stopped` → 0, `Failed` → `RUNTIME` (1 in the iter contract),
/// `Killed` → 130 (POSIX SIGINT-equivalent termination).
#[must_use]
pub fn status_exit_code(status: ProcessStatus) -> i32 {
    match status {
        ProcessStatus::Stopped => 0,
        ProcessStatus::Failed => exit_codes::RUNTIME,
        ProcessStatus::Killed => 130,
        // Non-terminal statuses are unreachable here (`attach` only
        // returns once `is_terminal()` is true) but mapping them to
        // `INTERNAL` keeps the function total.
        ProcessStatus::Initializing | ProcessStatus::Running => exit_codes::INTERNAL,
    }
}

async fn pump_log_ndjson(mut reader: NdjsonReader) {
    loop {
        match reader.next_entry().await {
            Ok(Some(entry)) => match entry.stream {
                LogStream::Stdout => {
                    let mut out = tokio::io::stdout();
                    drop(out.write_all(entry.line.as_bytes()).await);
                    drop(out.write_all(b"\n").await);
                    drop(out.flush().await);
                }
                LogStream::Stderr => {
                    let mut err = tokio::io::stderr();
                    drop(err.write_all(entry.line.as_bytes()).await);
                    drop(err.write_all(b"\n").await);
                    drop(err.flush().await);
                }
            },
            Ok(None) | Err(_) => return,
        }
    }
}

#[cfg(unix)]
fn install_signal_forwarder(notify: Arc<Notify>) {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let Ok(mut sigterm) = signal(SignalKind::terminate()) else {
            return;
        };
        let Ok(mut sigint) = signal(SignalKind::interrupt()) else {
            return;
        };
        loop {
            tokio::select! {
                _ = sigterm.recv() => notify.notify_one(),
                _ = sigint.recv() => notify.notify_one(),
            }
        }
    });
}

#[cfg(not(unix))]
fn install_signal_forwarder(notify: Arc<Notify>) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            notify.notify_one();
        }
    });
}
