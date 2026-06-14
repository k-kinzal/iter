//! Shell queue — escape hatch backend driven by user-supplied shell commands.
//!
//! Two halves work independently:
//!
//! * **Enqueue.** Each call to [`ShellQueue::enqueue`] runs the configured
//!   `enqueue` script via `<interpreter> <script>` (default `sh -c`). The
//!   serialized [`Envelope`](crate::queue::envelope::Envelope) is written to
//!   the child's stdin; a non-zero exit becomes
//!   [`ShellQueueError::EnqueueFailed`]. A 30-second default timeout
//!   (configurable via the queue declaration) reaps a stuck child's whole
//!   process tree via [`ProcessGroup`](crate::process_group::ProcessGroup)
//!   (SIGTERM, a short grace, then SIGKILL) so a runaway script cannot leak
//!   grandchildren.
//! * **Dequeue.** A long-lived child reads NDJSON signal records on its
//!   stdout. The reader task pushes parsed signals into an MPSC channel that
//!   [`ShellQueue::dequeue`] receives from. If the dequeue child exits before
//!   `close()`, the queue respawns it. On cancel or read failure the reader
//!   reaps the child's whole process tree through the same
//!   [`ProcessGroup`](crate::process_group::ProcessGroup) primitive the agent
//!   invocation uses.
//!
//! The dequeue NDJSON format supports
//! either a full envelope (`{"v":1,"signal":...,"priority":...}`) or the
//! short form `{"metadata": {...}, "priority": "..."}` whose `priority` is a
//! keyword (`low`/`normal`/`high`/`critical`) and whose `metadata` becomes a
//! freshly-generated signal. Lines that fail to parse are logged via stderr
//! and skipped — that matches the external-trigger behaviour for ad-hoc
//! scripts.

pub mod error;
pub mod settings;

pub use error::ShellQueueError;
pub use settings::ShellQueueConfig;

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::process_group::{self, ProcessGroup};
use crate::queue::QueueError;
use crate::queue::envelope::encode_signal;
use crate::signal::{Metadata, MetadataError, MetadataKey, MetadataValue, Signal};
use crate::{Priority, Queue};
use async_trait::async_trait;

/// SIGTERM-to-SIGKILL grace for a shell child's process tree. A stuck enqueue
/// or a cancelled dequeue is not expected to clean up gracefully, so this is
/// deliberately short — long enough for a well-behaved script to exit on
/// SIGTERM, short enough that cleanup is snappy.
const SHELL_TERMINATION_GRACE: Duration = Duration::from_millis(250);

/// Wire-shape of one NDJSON line emitted by the dequeue script.
///
/// Two flavours are accepted:
///
/// * Full [`Envelope`](crate::queue::envelope::Envelope): used when the
///   script wants total control over the signal (e.g. preserving an id from
///   the upstream broker).
/// * Short form: a `metadata` map plus optional `priority` keyword. iter
///   generates a fresh id and `created_at`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NdjsonLine {
    Envelope {
        v: u32,
        signal: Signal,
        #[serde(default)]
        priority: Option<Priority>,
    },
    Short {
        #[serde(default)]
        metadata: serde_json::Map<String, Value>,
        #[serde(default)]
        priority: Option<String>,
    },
}

#[derive(Debug)]
struct Inner {
    closed: bool,
    reader_task: Option<JoinHandle<()>>,
    reader_cancel: Option<CancellationToken>,
}

/// Shell-driven queue. See module docs for the contract.
#[derive(Clone, Debug)]
pub struct ShellQueue {
    config: Arc<ShellQueueConfig>,
    rx: Arc<Mutex<mpsc::Receiver<(Signal, Priority)>>>,
    inner: Arc<Mutex<Inner>>,
}

impl ShellQueue {
    /// Build the queue and spawn the long-lived dequeue reader task.
    ///
    /// # Errors
    ///
    /// Returns [`ShellQueueError::EmptyInterpreter`] when the configured
    /// interpreter string contains only whitespace.
    pub fn new(config: ShellQueueConfig) -> Result<Self, ShellQueueError> {
        if config.interpreter_argv().is_empty() {
            return Err(ShellQueueError::EmptyInterpreter);
        }
        let config = Arc::new(config);
        let (tx, rx) = mpsc::channel(64);
        let reader_cancel = CancellationToken::new();
        let task = tokio::spawn(reader_loop(Arc::clone(&config), tx, reader_cancel.clone()));
        Ok(Self {
            config,
            rx: Arc::new(Mutex::new(rx)),
            inner: Arc::new(Mutex::new(Inner {
                closed: false,
                reader_task: Some(task),
                reader_cancel: Some(reader_cancel),
            })),
        })
    }

    async fn run_enqueue(&self, payload: &[u8], priority: Priority) -> Result<(), ShellQueueError> {
        let argv = self.config.interpreter_argv();
        let (program, leading) = argv.split_first().expect("validated non-empty in `new`");
        let priority_name = priority.keyword();
        let mut command = Command::new(program);
        command
            .args(leading)
            .arg(&self.config.enqueue)
            .env("ITER_SIGNAL_PRIORITY", priority.value().to_string())
            .env("ITER_SIGNAL_PRIORITY_NAME", priority_name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Put the enqueue child in its own process group so a timeout reaps
        // the whole tree (the script plus any grandchildren it backgrounded),
        // not just the direct child.
        process_group::configure(&mut command);

        let mut child = command.spawn()?;
        let mut group = ProcessGroup::from_child(&child);

        if let Some(mut stdin) = child.stdin.take() {
            // A child that exits before reading (e.g. `exit 17`) closes stdin
            // first, so the write or shutdown returns BrokenPipe. That is not
            // an enqueue error — the exit status surfaces the real failure.
            match stdin.write_all(payload).await {
                Ok(()) => {
                    if let Err(e) = stdin.shutdown().await {
                        if e.kind() != std::io::ErrorKind::BrokenPipe {
                            return Err(ShellQueueError::Io(e));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(ShellQueueError::Io(e)),
            }
        }

        let timeout_dur = self.config.enqueue_timeout();
        match timeout(timeout_dur, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(ShellQueueError::EnqueueFailed {
                        status: output.status.code().unwrap_or(-1),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            }
            Ok(Err(e)) => Err(ShellQueueError::Io(e)),
            Err(_) => {
                // `wait_with_output` consumed `child`; on timeout its future
                // is dropped here, which hands the pid to tokio's background
                // reaper (so the direct child cannot become a lasting zombie).
                // We still SIGTERM→SIGKILL the whole group by its pgid so the
                // script's grandchildren are reaped too, not just the direct
                // child, before surfacing the timeout.
                group.terminate(SHELL_TERMINATION_GRACE).await;
                Err(ShellQueueError::EnqueueTimeout(timeout_dur))
            }
        }
    }
}

impl ShellQueue {
    async fn enqueue_signal(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> Result<(), ShellQueueError> {
        if self.inner.lock().await.closed {
            return Err(ShellQueueError::Closed);
        }
        let payload = encode_signal(&signal, priority);
        let mut payload = payload;
        // NDJSON-friendly: callers piping stdin into `jq` or similar appreciate
        // the trailing newline. The decoder ignores it.
        payload.push(b'\n');
        self.run_enqueue(&payload, priority).await
    }

    async fn dequeue_signal(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Signal>, ShellQueueError> {
        let mut rx = self.rx.lock().await;
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            value = rx.recv() => Ok(value.map(|(s, _p)| s)),
        }
    }

    async fn close_queue(&self) -> Result<(), ShellQueueError> {
        let mut inner = self.inner.lock().await;
        if inner.closed {
            return Ok(());
        }
        inner.closed = true;
        if let Some(token) = inner.reader_cancel.take() {
            token.cancel();
        }
        if let Some(handle) = inner.reader_task.take() {
            // Surrendering the lock would let a concurrent `dequeue` proceed;
            // we hold it because once the reader is gone the channel will
            // close and `dequeue` returns `Ok(None)` on the natural drain.
            drop(handle.await);
        }

        if let Some(close_script) = &self.config.close {
            let argv = self.config.interpreter_argv();
            let (program, leading) = argv.split_first().expect("validated non-empty in `new`");
            let mut command = Command::new(program);
            command
                .args(leading)
                .arg(close_script)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // Best-effort: a failing close script should not stop the runner
            // from terminating; surface the I/O error if spawn fails outright,
            // but ignore non-zero exits.
            let mut child = command.spawn()?;
            drop(child.wait().await);
        }

        Ok(())
    }
}

#[async_trait]
impl Queue for ShellQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.enqueue_signal(signal, priority)
            .await
            .map_err(QueueError::new)
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        self.dequeue_signal(cancel).await.map_err(QueueError::new)
    }

    async fn close(&self) -> Result<(), QueueError> {
        self.close_queue().await.map_err(QueueError::new)
    }
}

async fn reader_loop(
    config: Arc<ShellQueueConfig>,
    tx: mpsc::Sender<(Signal, Priority)>,
    cancel: CancellationToken,
) {
    let argv = config.interpreter_argv();
    let Some((program, leading)) = argv.split_first() else {
        return;
    };

    while !cancel.is_cancelled() {
        let mut command = Command::new(program);
        command
            .args(leading)
            .arg(&config.dequeue)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // Own the dequeue child by its process group so cancel/read-failure
        // reaps the whole tree, not just the direct child.
        process_group::configure(&mut command);

        let Ok(mut child) = command.spawn() else {
            // Spawn failure (binary missing, fork failure) — give the
            // user a chance to fix the interpreter without hot-spinning.
            if wait_or_cancel(&cancel, Duration::from_secs(1)).await {
                break;
            }
            continue;
        };
        let mut group = ProcessGroup::from_child(&child);

        let Some(stdout) = child.stdout.take() else {
            group.terminate(SHELL_TERMINATION_GRACE).await;
            drop(child.wait().await);
            continue;
        };
        let mut reader = BufReader::new(stdout).lines();

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    group.terminate(SHELL_TERMINATION_GRACE).await;
                    drop(child.wait().await);
                    return;
                }
                line = reader.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if text.trim().is_empty() {
                                continue;
                            }
                            if let Ok(parsed) = parse_ndjson_line(&text) {
                                if tx.send(parsed).await.is_err() {
                                    group.terminate(SHELL_TERMINATION_GRACE).await;
                                    drop(child.wait().await);
                                    return;
                                }
                            } else {
                                // Malformed line — skip rather than tear
                                // down the pipeline. Stderr inheritance
                                // surfaces script-side debug output.
                            }
                        }
                        Ok(None) => {
                            // EOF — child closed stdout. Reap and respawn.
                            // `group` drops here as a no-op safety net.
                            drop(child.wait().await);
                            break;
                        }
                        Err(_) => {
                            group.terminate(SHELL_TERMINATION_GRACE).await;
                            drop(child.wait().await);
                            break;
                        }
                    }
                }
            }
        }
    }
}

async fn wait_or_cancel(cancel: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(d) => false,
    }
}

fn parse_ndjson_line(text: &str) -> Result<(Signal, Priority), NdjsonError> {
    let parsed: NdjsonLine = serde_json::from_str(text)?;
    match parsed {
        NdjsonLine::Envelope {
            v,
            signal,
            priority,
        } => {
            if v != 1 {
                return Err(NdjsonError::UnsupportedVersion(v));
            }
            Ok((signal, priority.unwrap_or_default()))
        }
        NdjsonLine::Short { metadata, priority } => {
            let metadata = build_metadata(&metadata)?;
            let signal = Signal::new(metadata);
            let priority = priority
                .as_deref()
                .and_then(Priority::from_keyword)
                .unwrap_or_default();
            Ok((signal, priority))
        }
    }
}

#[derive(Debug, Error)]
enum NdjsonError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),
    #[error("unsupported envelope version {0}")]
    UnsupportedVersion(u32),
}

fn build_metadata(map: &serde_json::Map<String, Value>) -> Result<Metadata, MetadataError> {
    let mut metadata = Metadata::new();
    for (k, v) in map {
        let key = MetadataKey::new(k.as_str())?;
        let value = match v {
            Value::Null => MetadataValue::Null,
            Value::Bool(b) => MetadataValue::Bool(*b),
            Value::Number(n) => n.as_i64().map_or_else(
                || MetadataValue::String(n.to_string()),
                MetadataValue::Integer,
            ),
            Value::String(s) => MetadataValue::String(s.clone()),
            other @ (Value::Array(_) | Value::Object(_)) => {
                MetadataValue::String(other.to_string())
            }
        };
        metadata.insert(key, value);
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

    fn signal(label: &str) -> Signal {
        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("label").unwrap(),
            MetadataValue::String(label.into()),
        );
        Signal::new(metadata)
    }

    /// `kill(pid, 0)` liveness probe — returns `true` while the process
    /// exists, `false` once it has been reaped.
    #[cfg(unix)]
    fn pid_alive(pid: i32) -> bool {
        // SAFETY: `kill(pid, 0)` is the canonical liveness probe; signal 0
        // performs permission/existence checks without delivering.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    /// Poll `pid_alive` until it reports dead or the budget elapses.
    #[cfg(unix)]
    async fn wait_until_dead(pid: i32) -> bool {
        for _ in 0..100 {
            if !pid_alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        !pid_alive(pid)
    }

    /// Poll `path` until a script has written a pid into it.
    #[cfg(unix)]
    async fn read_recorded_pid(path: &std::path::Path) -> i32 {
        for _ in 0..50 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("script never recorded a grandchild pid");
    }

    #[test]
    fn ndjson_short_form_round_trip() {
        let line = r#"{"metadata": {"label": "alpha"}, "priority": "high"}"#;
        let (signal, priority) = parse_ndjson_line(line).expect("parse");
        assert_eq!(priority, Priority::HIGH);
        assert!(matches!(
            signal.metadata().get(&MetadataKey::new("label").unwrap()),
            Some(MetadataValue::String(s)) if s == "alpha"
        ));
    }

    #[test]
    fn ndjson_envelope_form_round_trip() {
        let s = signal("hi");
        let payload = encode_signal(&s, Priority::CRITICAL);
        let line = std::str::from_utf8(&payload).unwrap();
        let (back, priority) = parse_ndjson_line(line).expect("parse");
        assert_eq!(back, s);
        assert_eq!(priority, Priority::CRITICAL);
    }

    #[test]
    fn ndjson_unknown_envelope_version_rejected() {
        let line = r#"{"v": 99, "signal": {"id": "00000000-0000-0000-0000-000000000000", "created_at": "2026-01-01T00:00:00Z", "metadata": {}}, "priority": 50}"#;
        let err = parse_ndjson_line(line).expect_err("unknown version");
        assert!(matches!(err, NdjsonError::UnsupportedVersion(99)));
    }

    #[tokio::test]
    async fn enqueue_then_dequeue_round_trip_via_cat() {
        // `dequeue` re-emits whatever its stdin had; we wire it through a
        // FIFO-ish file so the enqueue produces NDJSON that the dequeue
        // reads. This exercises spawn+pipe glue end-to-end.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.ndjson");
        // Pre-create so `tail -F` does not spam stderr while the file is
        // missing; the test inspects stdout-only behaviour.
        std::fs::File::create(&path).unwrap();
        let path_str = path.to_string_lossy().into_owned();

        let config = ShellQueueConfig {
            enqueue: format!("cat >> {path_str}"),
            // Tail with `-F` keeps the file open and emits new lines as they
            // appear. `-n0` suppresses the back-history so the test only
            // sees what we explicitly enqueue.
            dequeue: format!("tail -n 0 -f {path_str}"),
            close: None,
            interpreter: None,
            enqueue_timeout: Some(Duration::from_secs(5)),
        };
        let q = ShellQueue::new(config).expect("new");

        let s = signal("hello");
        // Give the dequeue child time to open the file before the first
        // enqueue, otherwise tail's `-n 0` race window can swallow it.
        tokio::time::sleep(Duration::from_millis(200)).await;
        q.enqueue(s.clone(), Priority::HIGH).await.expect("enqueue");

        let cancel = CancellationToken::new();
        let recv = timeout(Duration::from_secs(5), q.dequeue(cancel))
            .await
            .expect("not timed out")
            .expect("ok");
        let recv = recv.expect("some");
        assert_eq!(recv, s);

        q.close().await.expect("close");
        let post = q
            .enqueue(signal("after-close"), Priority::NORMAL)
            .await
            .expect_err("closed rejects");
        assert!(matches!(
            post.downcast_ref::<ShellQueueError>(),
            Some(ShellQueueError::Closed)
        ));
    }

    #[tokio::test]
    async fn enqueue_failure_is_surfaced() {
        let config = ShellQueueConfig {
            enqueue: "exit 17".into(),
            dequeue: "true".into(),
            close: None,
            interpreter: None,
            enqueue_timeout: Some(Duration::from_secs(2)),
        };
        let q = ShellQueue::new(config).expect("new");
        let err = q
            .enqueue(signal("x"), Priority::NORMAL)
            .await
            .expect_err("non-zero exit");
        match err.downcast_ref::<ShellQueueError>() {
            Some(ShellQueueError::EnqueueFailed { status, .. }) => assert_eq!(*status, 17),
            other => panic!("unexpected: {other:?}"),
        }
        q.close().await.expect("close");
    }

    /// On enqueue timeout the stuck child's **whole process tree** must be
    /// reaped, not just the direct child — otherwise a script that backgrounds
    /// work leaks grandchildren reparented to init. The enqueue script
    /// backgrounds a `sleep`, records the grandchild pid, then blocks so the
    /// enqueue times out; afterwards the grandchild must be dead.
    #[cfg(unix)]
    #[tokio::test]
    async fn enqueue_timeout_reaps_stuck_child_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("grandchild.pid");
        let script = format!(
            "sleep 60 & echo $! > {}; wait",
            pidfile.to_str().expect("utf8 path")
        );
        let config = ShellQueueConfig {
            enqueue: script,
            dequeue: "true".into(),
            close: None,
            interpreter: None,
            enqueue_timeout: Some(Duration::from_millis(150)),
        };
        let q = ShellQueue::new(config).expect("new");
        let err = q
            .enqueue(signal("slow"), Priority::NORMAL)
            .await
            .expect_err("timeout");
        assert!(matches!(
            err.downcast_ref::<ShellQueueError>(),
            Some(ShellQueueError::EnqueueTimeout(_))
        ));

        // The script recorded the backgrounded grandchild's pid before
        // blocking; after the timeout reaped the group it must die.
        let grandchild = read_recorded_pid(&pidfile).await;
        assert!(
            wait_until_dead(grandchild).await,
            "grandchild should be reaped via the process group on enqueue timeout"
        );

        q.close().await.expect("close");
    }

    #[tokio::test]
    async fn empty_interpreter_rejected() {
        let config = ShellQueueConfig {
            enqueue: "true".into(),
            dequeue: "true".into(),
            close: None,
            interpreter: Some("   ".into()),
            enqueue_timeout: None,
        };
        let err = ShellQueue::new(config).expect_err("empty");
        assert!(matches!(err, ShellQueueError::EmptyInterpreter));
    }

    #[tokio::test]
    async fn cancel_unblocks_dequeue() {
        let config = ShellQueueConfig {
            enqueue: "true".into(),
            dequeue: "sleep 30".into(),
            close: None,
            interpreter: None,
            enqueue_timeout: None,
        };
        let q = ShellQueue::new(config).expect("new");
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let q2 = q.clone();
        let handle = tokio::spawn(async move { q2.dequeue(cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let result = timeout(Duration::from_secs(2), handle)
            .await
            .expect("not hung")
            .expect("join")
            .expect("ok");
        assert!(result.is_none());
        q.close().await.expect("close");
    }

    /// Cancelling the dequeue reader (via `close`) must reap the dequeue
    /// child's **whole process tree**, not just the direct child — the same
    /// guarantee the enqueue-timeout path makes. The dequeue script
    /// backgrounds a `sleep`, records its pid, then blocks; after `close`
    /// the grandchild must be dead.
    #[cfg(unix)]
    #[tokio::test]
    async fn dequeue_cancel_reaps_child_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("grandchild.pid");
        // Record the grandchild pid, then block forever on `wait` with no
        // output on stdout so the reader parks in `next_line()`.
        let script = format!(
            "sleep 60 & echo $! > {}; wait",
            pidfile.to_str().expect("utf8 path")
        );
        let config = ShellQueueConfig {
            enqueue: "true".into(),
            dequeue: script,
            close: None,
            interpreter: None,
            enqueue_timeout: None,
        };
        let q = ShellQueue::new(config).expect("new");

        // The reader spawned the dequeue child at construction; wait for the
        // backgrounded grandchild's pid to land.
        let grandchild = read_recorded_pid(&pidfile).await;
        assert!(
            pid_alive(grandchild),
            "grandchild should be alive pre-close"
        );

        // `close` cancels the reader, which reaps the dequeue child's group.
        q.close().await.expect("close");

        assert!(
            wait_until_dead(grandchild).await,
            "grandchild should be reaped via the process group on dequeue cancel"
        );
    }
}
