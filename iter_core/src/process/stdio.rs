//! `StdioSink` / `StdioPolicy` / `StdioSupervisor` — Process-side
//! capture of an Agent's standard streams into the unified
//! [`LOG_NDJSON`](crate::process::paths::names::LOG_NDJSON) stream.
//!
//! Per the docker-logs-parity design every byte the worker process
//! emits — agent stdout, agent stderr, runner tracing, lifecycle events —
//! lands as a `{ts, stream, line}` JSON record in `log.ndjson`. The two
//! policy modes the runtime owns are:
//!
//! - [`StdioPolicy::LogOnly`] — every spawned process. Bytes go through
//!   [`LogJsonSink`] into `log.ndjson`. Detached children additionally
//!   have their fd 1/2 bound to `/dev/null` by the spawner so any
//!   output that bypasses the in-process sink (e.g. a panic before the
//!   runtime is wired) is dropped silently rather than corrupting the
//!   NDJSON stream with raw non-JSON bytes.
//! - [`StdioPolicy::Passthrough`] — interactive agents (Claude Code,
//!   `codex chat`, …). The Agent inherits the TTY directly via
//!   `Stdio::inherit()` and the Process Runtime captures nothing. The
//!   sink is wired to a no-op so call-sites that always invoke
//!   [`StdioSink::write_stdout`] still type-check.
//!
//! ### Permissions
//!
//! `log.ndjson` is opened `O_CREAT|O_APPEND|O_WRONLY|O_CLOEXEC|O_NOFOLLOW`
//! with mode `0o600` per §B7/§B8.
//!
//! ### Concurrency
//!
//! [`LogJsonSink`] is a single mpsc + writer-task pipeline (same shape as
//! [`LifecycleObserver`](crate::process::observer::LifecycleObserver)).
//! Multiple producers (stdio pump tasks, the tracing subscriber's
//! [`MakeWriter`]) push complete lines through the channel; one writer
//! task drains, serialises to NDJSON, and appends to disk. This serialises
//! every concurrent write at the line boundary without a per-write file
//! flock.
//!
//! ### Pump
//!
//! [`StdioSupervisor::pump_stdout`] / [`StdioSupervisor::pump_stderr`]
//! return [`JoinHandle`]s. The runtime is expected to `await` them
//! during `finalize` (best-effort drain per §B6) so any tail bytes are
//! flushed before the terminal status is written.

use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::process::logs::{LogEntry, LogStream};
use crate::process::paths::names::LOG_NDJSON;

const PUMP_CHUNK_SIZE: usize = 8 * 1024;

/// Default in-flight capacity for the [`LogJsonSink`] mpsc channel.
///
/// Picked to match the [`LifecycleObserver`](crate::process::observer)
/// channel so a worker process holds two equally sized backlogs. The
/// async path applies back-pressure via `Sender::send().await`; the
/// sync (tracing) path is best-effort and drops on overflow.
pub const DEFAULT_LOG_BUFFER: usize = 1024;

/// Bound on how long [`LogJsonSink::flush`] blocks waiting for the writer
/// task to drain the channel and flush the underlying file.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// How a Process Runtime captures the Agent's stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioPolicy {
    /// Every captured process: write to `<log_dir>/log.ndjson` only.
    /// Foreground attach is implemented at the parent-CLI layer by
    /// tailing that file.
    LogOnly {
        /// Directory holding `log.ndjson`.
        log_dir: PathBuf,
    },
    /// Interactive agents that inherit the TTY directly. The Process
    /// Runtime captures nothing.
    Passthrough,
}

impl StdioPolicy {
    /// Borrow the log directory, when one is owned by this policy.
    #[must_use]
    pub fn log_dir(&self) -> Option<&Path> {
        match self {
            Self::LogOnly { log_dir } => Some(log_dir),
            Self::Passthrough => None,
        }
    }

    /// `true` when this policy writes anything to disk.
    #[must_use]
    pub fn writes_log_files(&self) -> bool {
        matches!(self, Self::LogOnly { .. })
    }
}

/// Async sink for Agent stdout/stderr chunks.
///
/// Implementations are expected to be cheap-to-clone (`Arc`) and free of
/// long-running blocking calls — local file appends and TTY writes only
/// per the design contract. Networked sinks would need additional
/// timeout/back-pressure work and are explicitly out of scope.
#[async_trait]
pub trait StdioSink: Send + Sync {
    /// Forward an owned chunk of stdout bytes.
    async fn write_stdout(&self, bytes: Bytes) -> io::Result<()>;
    /// Forward an owned chunk of stderr bytes.
    async fn write_stderr(&self, bytes: Bytes) -> io::Result<()>;
    /// Flush any buffered writes to the underlying medium.
    ///
    /// Called by [`crate::process::runtime::ProcessRuntime`] during
    /// `finalize` (best-effort drain per §B6) so any tail bytes left in
    /// the writer task's queue or the OS-level file buffer reach disk
    /// before the terminal status is written. The default
    /// implementation is a no-op for sinks that don't buffer.
    async fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    /// Flush the partial-line buffer for one stream only.
    ///
    /// Called from each agent-side tee task when its pipe reaches EOF
    /// (see `crate::agent::process::tee_lines`). At that point the
    /// counterpart stream may still be writing, so we must not flush
    /// its in-flight partial along with ours — that would split a
    /// still-mid-record stderr line into two NDJSON entries. The
    /// default implementation is a no-op (matches [`NoopSink`]); buffered
    /// sinks should override to drain only `stream`'s pending buffer.
    async fn flush_stream(&self, _stream: LogStream) -> io::Result<()> {
        Ok(())
    }
}

/// `StdioSink` that drops every byte. Used by [`StdioPolicy::Passthrough`].
#[derive(Debug, Default)]
pub struct NoopSink;

#[async_trait]
impl StdioSink for NoopSink {
    async fn write_stdout(&self, _bytes: Bytes) -> io::Result<()> {
        Ok(())
    }

    async fn write_stderr(&self, _bytes: Bytes) -> io::Result<()> {
        Ok(())
    }
}

/// One message in the [`LogJsonSink`] writer-task channel.
enum WriterMsg {
    /// Append a single NDJSON record to `log.ndjson`.
    Entry(LogEntry),
    /// Flush the underlying file and ack on the oneshot. Used by
    /// [`LogJsonSink::flush`] to give callers an "everything submitted
    /// so far is on disk" barrier.
    Flush(oneshot::Sender<io::Result<()>>),
}

/// `StdioSink` that appends `{ts, stream, line}` NDJSON records to a
/// single `log.ndjson` file via a dedicated writer task.
///
/// Producers (stdio pump tasks plus the tracing subscriber's
/// [`MakeWriter`]) push complete lines through an mpsc channel; the
/// writer task drains, serialises, and appends. Per-stream `pending_*`
/// buffers hold the partial trailing bytes between chunks so callers can
/// emit raw `Bytes` and rely on the sink to split on newlines.
pub struct LogJsonSink {
    sender: mpsc::Sender<WriterMsg>,
    pending_stdout: Mutex<Vec<u8>>,
    pending_stderr: Mutex<Vec<u8>>,
    /// Filled by the writer task when it aborts on an I/O error; read
    /// back on subsequent `BrokenPipe`s (channel-closed) so the original
    /// error reason isn't lost when the `JoinHandle` is dropped. Wrapped
    /// in `std::sync::Mutex` (not `tokio::sync`) because every access is
    /// a short critical section that does not await — both writer and
    /// reader sides hold it only long enough to read or replace the
    /// `Option<String>`.
    writer_error: WriterErrorSlot,
}

/// Shared between [`LogJsonSink`], [`LogJsonSender`], and the writer
/// task: lets producers reconstruct the writer's terminal `io::Error`
/// after its [`JoinHandle`] has been dropped.
type WriterErrorSlot = Arc<std::sync::Mutex<Option<String>>>;

impl std::fmt::Debug for LogJsonSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogJsonSink").finish_non_exhaustive()
    }
}

impl LogJsonSink {
    /// Open `<log_dir>/log.ndjson` for append, spawn the writer task,
    /// and return a sink ready for [`StdioSink`] dispatch and tracing
    /// fan-in.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the file open fails
    /// (typically `ENOENT` if `log_dir` does not exist or `EPERM` if
    /// the caller cannot write there).
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

    /// Clone the channel sender so other producers (notably the tracing
    /// subscriber's `MakeWriter`) can fan in to the same writer task.
    #[must_use]
    pub(crate) fn sender_handle(&self) -> LogJsonSender {
        LogJsonSender {
            sender: self.sender.clone(),
            writer_error: self.writer_error.clone(),
        }
    }

    /// Async-path send: applies back-pressure via `Sender::send().await`.
    async fn enqueue(&self, entry: LogEntry) -> io::Result<()> {
        self.sender
            .send(WriterMsg::Entry(entry))
            .await
            .map_err(|_| writer_dead_error(&self.writer_error, "log.ndjson writer task stopped"))
    }

    /// Append a chunk to the per-stream pending buffer, then drain every
    /// complete `\n`-terminated line into the writer task as a separate
    /// [`LogEntry`]. The trailing remainder (if any) stays in the
    /// pending buffer for the next chunk.
    ///
    /// The per-stream `pending_*` mutex is held across the enqueue loop
    /// so that concurrent callers writing to the *same* stream serialise
    /// at the line boundary. Without this, two callers could each drain
    /// their own complete lines from the buffer and then race on the
    /// channel send, allowing the second caller's lines to interleave
    /// ahead of the first caller's. Holding the lock across `await` is
    /// safe — `pending_stdout` and `pending_stderr` are independent, so
    /// stdout and stderr never block each other; only same-stream
    /// concurrent writes are serialised, which is the correct ordering.
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

    /// Drain one stream's pending partial-line buffer as a single final
    /// NDJSON record. Used by both [`Self::flush_partials`] (whole-sink
    /// drain at finalize) and [`StdioSink::flush_stream`] (per-pipe EOF
    /// in the agent tee).
    ///
    /// Holds the per-stream `pending_*` mutex across the `enqueue` await,
    /// mirroring [`Self::write_chunk`]. If we released the lock before
    /// enqueuing, a concurrent same-stream `write_chunk` could acquire
    /// the now-empty buffer, push a complete line, and land it on the
    /// channel ahead of this partial — inverting record order on disk.
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

    /// Drain both per-stream pending buffers as final lines. Called from
    /// [`StdioSink::flush`] — the partial may come from an agent that
    /// printed without a trailing newline before exiting.
    async fn flush_partials(&self) -> io::Result<()> {
        self.flush_partial_stream(LogStream::Stdout).await?;
        self.flush_partial_stream(LogStream::Stderr).await?;
        Ok(())
    }

    /// Round-trip a `WriterMsg::Flush` through the channel without
    /// touching the per-stream pending buffers. When the writer task
    /// pops the message, every previously enqueued `Entry` has already
    /// been appended; the writer then flushes the file and acks.
    ///
    /// Used directly by [`StdioSink::flush`] (after `flush_partials`)
    /// and by tests that need the writer task to drain enqueued entries
    /// to disk without forcing an on-buffer drain of the counterpart
    /// stream.
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
impl StdioSink for LogJsonSink {
    async fn write_stdout(&self, bytes: Bytes) -> io::Result<()> {
        self.write_chunk(LogStream::Stdout, &bytes).await
    }

    async fn write_stderr(&self, bytes: Bytes) -> io::Result<()> {
        self.write_chunk(LogStream::Stderr, &bytes).await
    }

    async fn flush(&self) -> io::Result<()> {
        // 1. Flush per-stream partial-line buffers as final lines.
        self.flush_partials().await?;
        // 2. Round-trip a Flush request through the channel.
        self.flush_writer_only().await
    }

    async fn flush_stream(&self, stream: LogStream) -> io::Result<()> {
        // Drain only this stream's pending partial. Do *not* round-trip
        // a global Flush — the counterpart pipe may still be appending,
        // and the runtime's finalize step will do the on-disk barrier.
        self.flush_partial_stream(stream).await
    }
}

/// Cheap, cloneable handle on the [`LogJsonSink`] writer-task channel.
///
/// The tracing subscriber's `MakeWriter` wiring lives in `iter_compose`
/// (where `tracing-subscriber` is a dependency) — it constructs a
/// [`LogJsonSender`] from the runtime's `LogJsonSink` and uses it to
/// push tracing-formatted lines into the same NDJSON pipeline as agent
/// stdio.
#[derive(Clone)]
pub struct LogJsonSender {
    sender: mpsc::Sender<WriterMsg>,
    writer_error: WriterErrorSlot,
}

impl LogJsonSender {
    /// Best-effort sync enqueue. Drops the line if the writer-task
    /// channel is full or the writer has stopped — appropriate for
    /// tracing's synchronous `MakeWriter` path where awaiting on
    /// back-pressure is not an option.
    pub fn try_send_line(&self, stream: LogStream, line: String) {
        drop(self.sender.try_send(WriterMsg::Entry(LogEntry {
            ts: Utc::now(),
            stream,
            line,
        })));
    }

    /// Async, back-pressured enqueue. Awaits channel capacity instead of
    /// dropping the line — appropriate for callers that have a tokio
    /// runtime in scope and need delivery guaranteed (e.g. the
    /// [`LifecycleObserver`](crate::process::observer::LifecycleObserver)
    /// writer task, where a dropped event would leave a hole in the
    /// post-mortem record).
    ///
    /// # Errors
    ///
    /// Returns `BrokenPipe` if the writer task has exited (the receiver
    /// half is dropped). The error message includes the writer task's
    /// terminal `io::Error` when one was recorded — see
    /// [`LogJsonSink::writer_error`]. The caller should log and continue
    /// — the lifecycle queue will be drained with no further log.ndjson
    /// writes.
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

/// Process-wide [`LogJsonSender`] used by the tracing subscriber's
/// `MakeWriter` to fan formatter lines into the per-process
/// `log.ndjson` once a [`crate::process::ProcessRuntime`] has been
/// constructed.
///
/// Set exactly once per process via [`install_global_log_sender`]; pre-set
/// reads return `None` so early tracing writes (before the runtime exists)
/// fall through to whatever console writer the subscriber also has wired.
static GLOBAL_LOG_SENDER: std::sync::OnceLock<LogJsonSender> = std::sync::OnceLock::new();

/// Publish the process-wide [`LogJsonSender`] for the tracing subscriber's
/// `MakeWriter` to use. Subsequent calls are ignored — the runtime is
/// constructed exactly once per worker process.
pub fn install_global_log_sender(sender: LogJsonSender) {
    drop(GLOBAL_LOG_SENDER.set(sender));
}

/// Borrow the process-wide [`LogJsonSender`] when one has been installed.
#[must_use]
pub fn global_log_sender() -> Option<&'static LogJsonSender> {
    GLOBAL_LOG_SENDER.get()
}

impl std::fmt::Debug for LogJsonSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogJsonSender").finish_non_exhaustive()
    }
}

/// Per-process orchestrator that owns the policy and the active
/// [`StdioSink`].
///
/// `StdioSupervisor` is the only place sinks are constructed. The runtime
/// hands [`Self::sink`] to [`crate::agent::Agent`] via
/// `AgentRunContext.stdio` and uses [`Self::pump_stdout`] /
/// [`Self::pump_stderr`] to drive the per-stream pump tasks when the
/// agent is configured with `Stdio::piped()`.
pub struct StdioSupervisor {
    policy: StdioPolicy,
    sink: Arc<dyn StdioSink>,
    /// `Some` for `LogOnly`. Cloned on demand by callers wiring tracing
    /// fan-in into the same NDJSON pipeline.
    log_sender: Option<LogJsonSender>,
}

impl StdioSupervisor {
    /// Build a supervisor matching the given [`StdioPolicy`].
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// Opens `log.ndjson` for `LogOnly`; for `Passthrough` no I/O
    /// happens.
    pub async fn new(policy: StdioPolicy) -> io::Result<Self> {
        let (sink, log_sender): (Arc<dyn StdioSink>, Option<LogJsonSender>) = match &policy {
            StdioPolicy::Passthrough => (Arc::new(NoopSink), None),
            StdioPolicy::LogOnly { log_dir } => {
                let sink = LogJsonSink::open_in(log_dir).await?;
                let sender = sink.sender_handle();
                (Arc::new(sink), Some(sender))
            }
        };
        Ok(Self {
            policy,
            sink,
            log_sender,
        })
    }

    /// Borrow the active [`StdioPolicy`].
    #[must_use]
    pub fn policy(&self) -> &StdioPolicy {
        &self.policy
    }

    /// Clone the [`Arc<dyn StdioSink>`] for distribution to agents and
    /// pump tasks.
    #[must_use]
    pub fn sink(&self) -> Arc<dyn StdioSink> {
        self.sink.clone()
    }

    /// Clone a sender into the underlying NDJSON writer task, when one
    /// exists. `None` for [`StdioPolicy::Passthrough`].
    #[must_use]
    pub fn log_sender(&self) -> Option<LogJsonSender> {
        self.log_sender.clone()
    }

    /// Spawn a pump task that copies `reader` chunks into
    /// [`StdioSink::write_stdout`].
    ///
    /// Returns a [`JoinHandle`] resolving to the first sink-write error,
    /// or `Ok(())` once `reader` reaches EOF. Reader read errors are
    /// returned wrapped in `Err`.
    pub fn pump_stdout<R>(&self, reader: R) -> JoinHandle<io::Result<()>>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let sink = self.sink.clone();
        tokio::spawn(async move { pump_into_sink(reader, sink, Direction::Stdout).await })
    }

    /// Spawn a pump task that copies `reader` chunks into
    /// [`StdioSink::write_stderr`]. Mirror of [`Self::pump_stdout`].
    pub fn pump_stderr<R>(&self, reader: R) -> JoinHandle<io::Result<()>>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let sink = self.sink.clone();
        tokio::spawn(async move { pump_into_sink(reader, sink, Direction::Stderr).await })
    }
}

#[derive(Copy, Clone)]
enum Direction {
    Stdout,
    Stderr,
}

async fn pump_into_sink<R>(
    mut reader: R,
    sink: Arc<dyn StdioSink>,
    direction: Direction,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buf = vec![0u8; PUMP_CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        let chunk = Bytes::copy_from_slice(&buf[..n]);
        match direction {
            Direction::Stdout => sink.write_stdout(chunk).await?,
            Direction::Stderr => sink.write_stderr(chunk).await?,
        }
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

/// Persist the writer task's terminal `io::Error` so producers can
/// reconstruct it after the channel has closed (since the
/// [`JoinHandle`] is dropped). Also emits a `tracing::error!` for
/// foreground stderr visibility — the same target is filtered out of
/// the log.ndjson tracing layer to avoid recursing into the very channel
/// that just broke.
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

/// Build a `BrokenPipe` `io::Error` that, when the writer task already
/// recorded a terminal error, includes that error's text in the
/// message. Lets producers see the original cause (`ENOSPC`, etc.)
/// instead of just "writer task stopped".
fn writer_dead_error(slot: &WriterErrorSlot, fallback: &str) -> io::Error {
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
    // Use the synchronous OpenOptions so we can set custom_flags for
    // O_CLOEXEC|O_NOFOLLOW + mode(0o600). tokio::fs::OpenOptions exposes
    // mode but not custom_flags as of tokio 1.x.
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
        let s = NoopSink;
        s.write_stdout(Bytes::from_static(b"hello"))
            .await
            .expect("ok");
        s.write_stderr(Bytes::from_static(b"world"))
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn log_json_sink_emits_one_entry_per_line() {
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
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
    async fn log_json_sink_buffers_partial_lines_across_chunks() {
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
        sink.write_stdout(Bytes::from_static(b"abc"))
            .await
            .expect("a");
        sink.write_stdout(Bytes::from_static(b"def\nghi"))
            .await
            .expect("b");
        sink.flush().await.expect("flush");

        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        // Two complete lines on stdout: "abcdef" and the trailing "ghi"
        // flushed by `flush_partials`.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].line, "abcdef");
        assert_eq!(entries[1].line, "ghi");
    }

    #[tokio::test]
    async fn log_json_sink_strips_crlf() {
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
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
    async fn log_json_sink_separates_stdout_and_stderr_partials() {
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
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
        // Per-stream partials must not interleave.
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
        // Per Codex round-3 finding 1: stdout EOF must not drag the
        // counterpart stderr partial out of its pending buffer mid-record.
        //
        // Scenario A — disk-state observation: with stdout and stderr
        // both holding partial bytes, flush_stream(Stdout) followed by
        // a writer-only round-trip (which does NOT call flush_partials)
        // must reveal exactly the stdout partial on disk; the stderr
        // partial must remain in its in-memory buffer.
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
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

        // Scenario B — semantic invariant: an undrained stderr partial
        // still in the buffer must merge with subsequent stderr writes
        // into a single NDJSON record. If flush_stream(Stdout) had
        // erroneously drained stderr's partial, "still-active" would be
        // its own record and " continued" would land separately.
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
        // Per Codex round-3 finding 2: when run_writer aborts on an
        // io::Error, subsequent send/flush calls should surface the
        // original error reason instead of just "writer task stopped".
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
        // The slot is empty when the writer task exits cleanly (channel
        // closed normally) rather than aborting on I/O error. Producers
        // that race the shutdown still get a stable BrokenPipe with the
        // fallback message.
        let slot: WriterErrorSlot = Arc::new(std::sync::Mutex::new(None));
        let surfaced = writer_dead_error(&slot, "log.ndjson writer task stopped");
        assert_eq!(surfaced.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(surfaced.to_string(), "log.ndjson writer task stopped");
    }

    #[tokio::test]
    async fn log_json_sink_open_fails_for_symlink_target() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, b"").unwrap();
        let link = dir.path().join(LOG_NDJSON);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = LogJsonSink::open_in(dir.path())
            .await
            .expect_err("nofollow");
        assert!(
            err.raw_os_error().is_some(),
            "expected OS error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn log_json_sender_try_send_routes_through_writer_task() {
        let dir = TempDir::new().unwrap();
        let sink = LogJsonSink::open_in(dir.path()).await.expect("open");
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

    #[tokio::test]
    async fn supervisor_for_passthrough_uses_noop_sink() {
        let s = StdioSupervisor::new(StdioPolicy::Passthrough)
            .await
            .expect("new");
        assert!(matches!(s.policy(), StdioPolicy::Passthrough));
        assert!(!s.policy().writes_log_files());
        assert!(s.log_sender().is_none());
        s.sink().write_stdout(Bytes::new()).await.expect("ok");
    }

    #[tokio::test]
    async fn supervisor_for_log_only_opens_files_and_exposes_sender() {
        let dir = TempDir::new().unwrap();
        let s = StdioSupervisor::new(StdioPolicy::LogOnly {
            log_dir: dir.path().to_owned(),
        })
        .await
        .expect("new");
        assert!(s.policy().writes_log_files());
        s.sink()
            .write_stdout(Bytes::from_static(b"x\n"))
            .await
            .expect("ok");
        s.sink().flush().await.expect("flush");
        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, "x");
        assert!(s.log_sender().is_some());
    }

    #[tokio::test]
    async fn pump_stdout_forwards_chunks_to_sink_until_eof() {
        let dir = TempDir::new().unwrap();
        let sup = StdioSupervisor::new(StdioPolicy::LogOnly {
            log_dir: dir.path().to_owned(),
        })
        .await
        .expect("new");

        let (mut writer, reader) = tokio::io::duplex(64);
        let pump = sup.pump_stdout(reader);
        writer.write_all(b"hello\n").await.unwrap();
        writer.write_all(b"world\n").await.unwrap();
        drop(writer); // EOF
        let res = pump.await.expect("join");
        assert!(res.is_ok());
        sup.sink().flush().await.expect("flush");
        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        let lines: Vec<&str> = entries.iter().map(|e| e.line.as_str()).collect();
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[tokio::test]
    async fn pump_stderr_forwards_chunks_independently() {
        let dir = TempDir::new().unwrap();
        let sup = StdioSupervisor::new(StdioPolicy::LogOnly {
            log_dir: dir.path().to_owned(),
        })
        .await
        .expect("new");

        let (mut writer, reader) = tokio::io::duplex(64);
        let pump = sup.pump_stderr(reader);
        writer.write_all(b"oops\n").await.unwrap();
        drop(writer);
        pump.await.expect("join").expect("ok");
        sup.sink().flush().await.expect("flush");
        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].stream, LogStream::Stderr);
        assert_eq!(entries[0].line, "oops");
    }

    #[tokio::test]
    async fn pump_does_not_deadlock_on_large_input() {
        // Larger than PUMP_CHUNK_SIZE so multiple iterations run. The
        // payload is printable ASCII because `LogEntry.line` is a JSON
        // string and non-UTF-8 bytes would be lossily replaced — which
        // is fine in production (worker output is text) but obscures
        // the deadlock check this test cares about.
        let payload: Vec<u8> = (0..(PUMP_CHUNK_SIZE * 3 + 17))
            .map(|i| b'!' + u8::try_from(i % 90).expect("90 fits"))
            .collect();
        let dir = TempDir::new().unwrap();
        let sup = StdioSupervisor::new(StdioPolicy::LogOnly {
            log_dir: dir.path().to_owned(),
        })
        .await
        .expect("new");
        let (mut writer, reader) = tokio::io::duplex(64);
        let pump = sup.pump_stdout(reader);
        let payload_for_writer = payload.clone();
        let writer_handle = tokio::spawn(async move {
            writer.write_all(&payload_for_writer).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
            drop(writer);
        });
        writer_handle.await.expect("writer");
        pump.await.expect("join").expect("ok");
        sup.sink().flush().await.expect("flush");
        let entries = read_entries(&dir.path().join(LOG_NDJSON));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line.as_bytes(), payload.as_slice());
    }
}
