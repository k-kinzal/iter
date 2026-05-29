//! Cloneable channel handle for fan-in to the NDJSON writer task.

use std::io;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::log::{LogEntry, LogStream, WriterErrorSlot, WriterMsg, writer_dead_error};

/// Cheap, cloneable handle on the [`ProcessLogSink`](super::sink::ProcessLogSink)
/// writer-task channel.
///
/// The tracing subscriber's `MakeWriter` wiring lives in `iter_compose`
/// — it uses a `LogSender` to push tracing-formatted lines into the same
/// NDJSON pipeline as agent stdio.
#[derive(Clone)]
pub struct LogSender {
    pub(super) sender: mpsc::Sender<WriterMsg>,
    pub(super) writer_error: WriterErrorSlot,
}

impl LogSender {
    /// Best-effort sync enqueue. Drops the line if the writer-task
    /// channel is full or the writer has stopped.
    pub fn try_send_line(&self, stream: LogStream, line: String) {
        drop(self.sender.try_send(WriterMsg::Entry(LogEntry {
            ts: Utc::now(),
            stream,
            line,
        })));
    }

    /// Async, back-pressured enqueue. Awaits channel capacity instead of
    /// dropping the line.
    ///
    /// # Errors
    ///
    /// Returns `BrokenPipe` if the writer task has exited.
    pub async fn send_line(&self, stream: LogStream, line: String) -> io::Result<()> {
        self.sender
            .send(WriterMsg::Entry(LogEntry {
                ts: Utc::now(),
                stream,
                line,
            }))
            .await
            .map_err(|_| writer_dead_error(&self.writer_error, "log.ndjson writer task stopped"))
    }
}

impl std::fmt::Debug for LogSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogSender").finish_non_exhaustive()
    }
}

/// Process-wide [`LogSender`] used by the tracing subscriber's
/// `MakeWriter` to fan formatter lines into the per-process
/// `log.ndjson`.
static GLOBAL_LOG_SENDER: std::sync::OnceLock<LogSender> = std::sync::OnceLock::new();

/// Publish the process-wide [`LogSender`] for the tracing subscriber's
/// `MakeWriter` to use. Subsequent calls are ignored.
pub fn install_global_log_sender(sender: LogSender) {
    drop(GLOBAL_LOG_SENDER.set(sender));
}

/// Borrow the process-wide [`LogSender`] when one has been installed.
#[must_use]
pub fn global_log_sender() -> Option<&'static LogSender> {
    GLOBAL_LOG_SENDER.get()
}
