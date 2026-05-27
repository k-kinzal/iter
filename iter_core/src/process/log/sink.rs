//! `ProcessLogSink` — mpsc + writer task pipeline that appends NDJSON
//! records to `log.ndjson`.

use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::log::{LogEntry, LogStream, OutputSink};
use crate::process::paths::names::LOG_NDJSON;

use super::sender::LogSender;

/// Default in-flight capacity for the mpsc channel.
pub const DEFAULT_LOG_BUFFER: usize = 1024;

const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// One message in the writer-task channel.
pub(super) enum WriterMsg {
    Entry(LogEntry),
    Flush(oneshot::Sender<io::Result<()>>),
}

/// Shared error slot between sink, sender, and writer task.
pub(super) type WriterErrorSlot = Arc<std::sync::Mutex<Option<String>>>;

/// Sink that appends `{ts, stream, line}` NDJSON records to a single
/// `log.ndjson` file via a dedicated writer task. Implements
/// [`OutputSink`].
pub struct ProcessLogSink {
    sender: mpsc::Sender<WriterMsg>,
    pending_stdout: Mutex<Vec<u8>>,
    pending_stderr: Mutex<Vec<u8>>,
    writer_error: WriterErrorSlot,
}

impl std::fmt::Debug for ProcessLogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessLogSink").finish_non_exhaustive()
    }
}

impl ProcessLogSink {
    /// Open `<log_dir>/log.ndjson` for append, spawn the writer task.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the file open fails.
    pub async fn open_in(log_dir: &Path) -> io::Result<Self> {
        let path = log_dir.join(LOG_NDJSON);
        let file = open_log_file(path).await?;
        let (tx, rx) = mpsc::channel::<WriterMsg>(DEFAULT_LOG_BUFFER);
        let writer_error: WriterErrorSlot = Arc::new(std::sync::Mutex::new(None));
        tokio::spawn(run_writer(file, rx, writer_error.clone()));
        Ok(Self {
            sender: tx,
            pending_stdout: Mutex::new(Vec::new()),
            pending_stderr: Mutex::new(Vec::new()),
            writer_error,
        })
    }

    /// Clone the channel sender so other producers can fan in to the
    /// same writer task.
    #[must_use]
    pub(crate) fn sender_handle(&self) -> LogSender {
        LogSender {
            sender: self.sender.clone(),
            writer_error: self.writer_error.clone(),
        }
    }

    async fn enqueue(&self, entry: LogEntry) -> io::Result<()> {
        self.sender
            .send(WriterMsg::Entry(entry))
            .await
            .map_err(|_| writer_dead_error(&self.writer_error, "log.ndjson writer task stopped"))
    }

    async fn write_chunk(&self, stream: LogStream, bytes: &[u8]) -> io::Result<()> {
        let pending = match stream {
            LogStream::Stdout => &self.pending_stdout,
            LogStream::Stderr => &self.pending_stderr,
        };
        let mut lock = pending.lock().await;
        lock.extend_from_slice(bytes);
        let mut lines: Vec<String> = Vec::new();
        let mut start = 0usize;
        while let Some(rel) = lock[start..].iter().position(|b| *b == b'\n') {
            let end = start + rel;
            let slice_end = if end > start && lock[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            lines.push(String::from_utf8_lossy(&lock[start..slice_end]).into_owned());
            start = end + 1;
        }
        if start > 0 {
            lock.drain(..start);
        }
        for line in lines {
            self.enqueue(LogEntry {
                ts: Utc::now(),
                stream,
                line,
            })
            .await?;
        }
        Ok(())
    }

    async fn flush_partial_stream(&self, stream: LogStream) -> io::Result<()> {
        let pending = match stream {
            LogStream::Stdout => &self.pending_stdout,
            LogStream::Stderr => &self.pending_stderr,
        };
        let mut lock = pending.lock().await;
        if lock.is_empty() {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&lock).into_owned();
        lock.clear();
        self.enqueue(LogEntry {
            ts: Utc::now(),
            stream,
            line,
        })
        .await?;
        Ok(())
    }

    async fn flush_partials(&self) -> io::Result<()> {
        self.flush_partial_stream(LogStream::Stdout).await?;
        self.flush_partial_stream(LogStream::Stderr).await?;
        Ok(())
    }

    /// Round-trip a flush through the writer task channel without
    /// touching per-stream pending buffers.
    async fn flush_writer_only(&self) -> io::Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.sender
            .send(WriterMsg::Flush(ack_tx))
            .await
            .map_err(|_| writer_dead_error(&self.writer_error, "log.ndjson writer task stopped"))?;
        match tokio::time::timeout(FLUSH_TIMEOUT, ack_rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(writer_dead_error(
                &self.writer_error,
                "log.ndjson writer task disappeared during flush",
            )),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "log.ndjson writer task did not flush within the deadline",
            )),
        }
    }
}

#[async_trait]
impl OutputSink for ProcessLogSink {
    async fn write_stdout(&self, bytes: Bytes) -> io::Result<()> {
        self.write_chunk(LogStream::Stdout, &bytes).await
    }

    async fn write_stderr(&self, bytes: Bytes) -> io::Result<()> {
        self.write_chunk(LogStream::Stderr, &bytes).await
    }

    async fn flush(&self) -> io::Result<()> {
        self.flush_partials().await?;
        self.flush_writer_only().await
    }

    async fn flush_stream(&self, stream: LogStream) -> io::Result<()> {
        self.flush_partial_stream(stream).await
    }
}

// --- writer task internals ---

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
    let description = format!("{err}");
    if let Ok(mut guard) = slot.lock() {
        if guard.is_none() {
            *guard = Some(description.clone());
        }
    }
    tracing::error!(
        target: "iter::log",
        error = %err,
        kind = ?err.kind(),
        "{context}"
    );
}

pub(super) fn writer_dead_error(slot: &WriterErrorSlot, fallback: &str) -> io::Error {
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
    use tempfile::TempDir;

    fn read_entries(path: &Path) -> Vec<LogEntry> {
        let body = std::fs::read_to_string(path).unwrap();
        body.lines()
            .map(|l| serde_json::from_str::<LogEntry>(l).expect("parse"))
            .collect()
    }

    #[tokio::test]
    async fn noop_sink_drops_bytes() {
        use crate::log::NoopSink;
        let s = NoopSink;
        s.write_stdout(Bytes::from_static(b"hello"))
            .await
            .expect("ok");
        s.write_stderr(Bytes::from_static(b"world"))
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn process_log_sink_emits_one_entry_per_line() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"first\nsecond\n"))
            .await
            .expect("write");
        sink.write_stderr(Bytes::from_static(b"warn one\n"))
            .await
            .expect("write");
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].stream, LogStream::Stdout);
        assert_eq!(entries[0].line, "first");
        assert_eq!(entries[1].stream, LogStream::Stdout);
        assert_eq!(entries[1].line, "second");
        assert_eq!(entries[2].stream, LogStream::Stderr);
        assert_eq!(entries[2].line, "warn one");
    }

    #[tokio::test]
    async fn process_log_sink_buffers_partial_lines_across_chunks() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"abc"))
            .await
            .expect("a");
        sink.write_stdout(Bytes::from_static(b"def\nghi"))
            .await
            .expect("b");
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].line, "abcdef");
        assert_eq!(entries[1].line, "ghi");
    }

    #[tokio::test]
    async fn process_log_sink_strips_crlf() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"win\r\nunix\n"))
            .await
            .expect("write");
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].line, "win");
        assert_eq!(entries[1].line, "unix");
    }

    #[tokio::test]
    async fn process_log_sink_separates_stdout_and_stderr_partials() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"stdo"))
            .await
            .expect("a");
        sink.write_stderr(Bytes::from_static(b"stde"))
            .await
            .expect("b");
        sink.write_stdout(Bytes::from_static(b"ut\n"))
            .await
            .expect("c");
        sink.write_stderr(Bytes::from_static(b"rr\n"))
            .await
            .expect("d");
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 2);
        let stdout_line = entries
            .iter()
            .find(|e| e.stream == LogStream::Stdout)
            .expect("stdout entry");
        let stderr_line = entries
            .iter()
            .find(|e| e.stream == LogStream::Stderr)
            .expect("stderr entry");
        assert_eq!(stdout_line.line, "stdout");
        assert_eq!(stderr_line.line, "stderr");
    }

    #[tokio::test]
    async fn flush_stream_drains_only_targeted_stream_partial() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"stdout-final"))
            .await
            .expect("stdout partial");
        sink.write_stderr(Bytes::from_static(b"stderr-still-active"))
            .await
            .expect("stderr partial");

        sink.flush_stream(LogStream::Stdout)
            .await
            .expect("flush stdout");
        sink.flush_writer_only().await.expect("writer-only flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(
            entries.len(),
            1,
            "only the stdout partial should be on disk; stderr must still be pending"
        );
        assert_eq!(entries[0].stream, LogStream::Stdout);
        assert_eq!(entries[0].line, "stdout-final");

        sink.write_stderr(Bytes::from_static(b" continued\n"))
            .await
            .expect("stderr continued");
        sink.flush().await.expect("global flush");
        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        let stderr_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.stream == LogStream::Stderr)
            .collect();
        assert_eq!(
            stderr_entries.len(),
            1,
            "stderr partial should merge with subsequent write into one record"
        );
        assert_eq!(stderr_entries[0].line, "stderr-still-active continued");
    }

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

    #[tokio::test]
    async fn process_log_sink_open_fails_for_symlink_target() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, b"").unwrap();
        let link = dir.path().join(LOG_NDJSON);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ProcessLogSink::open_in(dir.path())
            .await
            .expect_err("nofollow");
        assert!(
            err.raw_os_error().is_some(),
            "expected OS error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn log_sender_try_send_routes_through_writer_task() {
        let dir = TempDir::new().unwrap();
        let sink = ProcessLogSink::open_in(dir.path()).await.expect("open");
        let sender = sink.sender_handle();
        sender.try_send_line(LogStream::Stderr, "tracing-line".into());
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert!(
            entries
                .iter()
                .any(|e| e.stream == LogStream::Stderr && e.line == "tracing-line")
        );
    }
}
