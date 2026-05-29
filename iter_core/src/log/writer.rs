//! `NdjsonWriter` — an mpsc + writer-task pipeline that appends
//! [`LogEntry`] records to a file as NDJSON lines.
//!
//! It has no dependency on `crate::process`, which keeps the generic
//! NDJSON serialization out of the process layer; its diagnostics name
//! `log.ndjson`, its sole caller today. Process-side framing (line
//! splitting, partial buffers, the global sender) lives in
//! [`crate::process::log`].

use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use super::LogEntry;

/// One message in the writer-task channel.
pub(crate) enum WriterMsg {
    /// Append one record as a single NDJSON line.
    Entry(LogEntry),
    /// Flush buffered writes, acknowledging the result through the
    /// oneshot channel.
    Flush(oneshot::Sender<io::Result<()>>),
}

/// Shared error slot recording the first fatal writer-task error.
///
/// Read back by [`writer_dead_error`] so a send onto a dead channel can
/// surface the original I/O cause instead of an opaque `BrokenPipe`.
pub(crate) type WriterErrorSlot = Arc<std::sync::Mutex<Option<String>>>;

/// Owns the NDJSON writer-task channel and its shared error slot.
///
/// Construction opens the target file for append and spawns the writer
/// task. Producers fan in by cloning the [`sender`](Self::sender); a
/// dead task is detected through the [`error_slot`](Self::error_slot).
pub(crate) struct NdjsonWriter {
    sender: mpsc::Sender<WriterMsg>,
    error_slot: WriterErrorSlot,
}

impl NdjsonWriter {
    /// Open `path` for append and spawn the writer task backed by an
    /// mpsc channel of `buffer` in-flight messages.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the file open fails.
    pub(crate) async fn open(path: PathBuf, buffer: usize) -> io::Result<Self> {
        let file = open_log_file(path).await?;
        let (tx, rx) = mpsc::channel::<WriterMsg>(buffer);
        let error_slot: WriterErrorSlot = Arc::new(std::sync::Mutex::new(None));
        tokio::spawn(run_writer(file, rx, error_slot.clone()));
        Ok(Self {
            sender: tx,
            error_slot,
        })
    }

    /// Borrow the channel sender, for enqueuing directly or cloning into
    /// additional producers.
    #[must_use]
    pub(crate) fn sender(&self) -> &mpsc::Sender<WriterMsg> {
        &self.sender
    }

    /// Borrow the shared error slot for dead-task diagnosis.
    #[must_use]
    pub(crate) fn error_slot(&self) -> &WriterErrorSlot {
        &self.error_slot
    }
}

impl std::fmt::Debug for NdjsonWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdjsonWriter").finish_non_exhaustive()
    }
}

async fn run_writer(
    mut file: TokioFile,
    mut rx: mpsc::Receiver<WriterMsg>,
    error_slot: WriterErrorSlot,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            WriterMsg::Entry(entry) => {
                let mut line = match serde_json::to_vec(&entry) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "log.ndjson entry dropped: JSON serialise failed"
                        );
                        continue;
                    }
                };
                line.push(b'\n');
                if let Err(e) = file.write_all(&line).await {
                    record_writer_error(
                        &error_slot,
                        &e,
                        "log.ndjson writer task aborting on write_all",
                    );
                    return;
                }
            }
            WriterMsg::Flush(ack) => {
                let res = file.flush().await;
                drop(ack.send(res));
            }
        }
    }
    if let Err(e) = file.flush().await {
        record_writer_error(
            &error_slot,
            &e,
            "log.ndjson writer task final flush failed at shutdown",
        );
    }
}

fn record_writer_error(slot: &WriterErrorSlot, err: &io::Error, context: &str) {
    if let Ok(mut guard) = slot.lock() {
        if guard.is_none() {
            *guard = Some(err.to_string());
        }
    }
    tracing::error!(
        target: "iter::log",
        error = %err,
        kind = ?err.kind(),
        "{context}"
    );
}

/// Build the `BrokenPipe` error returned when the writer-task channel is
/// closed, enriching it with the recorded original cause when present.
pub(crate) fn writer_dead_error(slot: &WriterErrorSlot, fallback: &str) -> io::Error {
    let recorded = slot.lock().ok().and_then(|g| g.clone());
    match recorded {
        Some(orig) => io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("{fallback}: writer task aborted: {orig}"),
        ),
        None => io::Error::new(io::ErrorKind::BrokenPipe, fallback.to_string()),
    }
}

async fn open_log_file(path: PathBuf) -> io::Result<TokioFile> {
    let std_file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&path)
    })
    .await
    .map_err(|e| io::Error::other(format!("spawn_blocking: {e}")))??;
    Ok(TokioFile::from_std(std_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writer_dead_error_surfaces_recorded_writer_io_error() {
        let slot: WriterErrorSlot = Arc::new(std::sync::Mutex::new(None));
        let original = io::Error::new(io::ErrorKind::StorageFull, "ENOSPC: disk full");
        record_writer_error(&slot, &original, "log.ndjson writer aborting (test)");
        let surfaced = writer_dead_error(&slot, "log.ndjson writer task stopped");
        assert_eq!(surfaced.kind(), io::ErrorKind::BrokenPipe);
        let msg = surfaced.to_string();
        assert!(
            msg.contains("ENOSPC"),
            "expected original cause in surfaced message, got: {msg}"
        );
        assert!(
            msg.contains("disk full"),
            "expected original detail in surfaced message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn writer_dead_error_falls_back_when_slot_empty() {
        let slot: WriterErrorSlot = Arc::new(std::sync::Mutex::new(None));
        let surfaced = writer_dead_error(&slot, "log.ndjson writer task stopped");
        assert_eq!(surfaced.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(surfaced.to_string(), "log.ndjson writer task stopped");
    }
}
