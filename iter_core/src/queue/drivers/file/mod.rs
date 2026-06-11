//! Persistent directory-of-files [`Queue`] implementation.
//!
//! [`FileQueue`] stores each signal as a single JSON file inside a
//! per-queue directory. Ordering and FIFO are encoded into the filename so
//! that lex-sorting the directory yields the next signal to dequeue. The
//! atomicity guarantees that make the whole thing a queue come from POSIX
//! `rename(2)`, which is the same primitive Maildir / qmail / Postfix have
//! relied on for decades:
//!
//! - **Enqueue:** write the payload into `tmp/<name>.partial`, fsync it,
//!   then `rename` it into `pending/<name>`. The rename either succeeds
//!   atomically or fails; a partial file never appears in `pending/`.
//! - **Dequeue:** scan `pending/` for the lex-smallest entry and `rename`
//!   it into `tmp/<name>.claim-<pid>-<uniq>`. The rename is the
//!   serialization point — exactly one consumer wins per file even when
//!   multiple processes share the directory.
//!
//! Filenames look like
//! `{255-priority:03}-{nanos:020}-{seq:010}-{signal_id}.json`, which makes
//! lex order match `(priority desc, time asc, seq asc)`.
//!
//! Crash semantics: enqueue is durable once the publishing rename plus
//! its directory fsync return, and a dequeue that crashes between the
//! claim-rename and the unlink loses the in-flight signal — the orphan
//! `.claim-*` file is swept on the next [`FileQueue::open`].

pub mod error;
pub mod layout;

pub use error::FileQueueError;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::queue::QueueError;
use crate::{Priority, Queue, Signal};
use async_trait::async_trait;
use notify::{Config as NotifyConfig, PollWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use layout::{CLAIM_INFIX, PARTIAL_SUFFIX, PENDING_DIR, POLL_INTERVAL, TMP_DIR};

/// Persistent priority queue backed by a directory of JSON files and POSIX
/// `rename(2)`.
///
/// Cheap to clone via the inner [`Arc`]: every clone shares the same
/// in-process notifier and `notify` watcher. Multiple [`FileQueue`]
/// instances — either in the same process or across processes — may point
/// at the same directory; the rename-based claim protocol guarantees that
/// each signal is handed to exactly one dequeue caller.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::{Metadata, Priority, Queue, Signal};
/// use iter_core::queue::FileQueue;
///
/// let queue = FileQueue::open("./queue")?;
/// queue.enqueue(Signal::new(Metadata::new()), Priority::HIGH).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct FileQueue {
    inner: Arc<Inner>,
}

struct Inner {
    /// Root directory passed to [`FileQueue::open`].
    root: PathBuf,
    /// `<root>/pending` — the lex-ordered set of ready signals.
    pending: PathBuf,
    /// `<root>/tmp` — scratch for in-flight enqueues and dequeues.
    tmp: PathBuf,
    /// Per-process monotonic sequence used to break ties between filename
    /// timestamps and to disambiguate concurrent claim filenames within a
    /// single process.
    next_seq: AtomicU64,
    /// Set by [`Queue::close`]. Once `true`, further enqueues are
    /// rejected with [`FileQueueError::Closed`] and a drained `pending/`
    /// makes `dequeue` return `Ok(None)`.
    closed: AtomicBool,
    /// Wakes parked `dequeue` calls. Shared with the watcher closure via
    /// `Arc<Notify>` so that filesystem events triggered by another
    /// process flow through the same primitive as in-process producers.
    notify: Arc<Notify>,
    /// Live filesystem watcher. Held only to keep the watch alive for
    /// the lifetime of the queue; the watcher delivers events directly
    /// into `notify` via its callback. We pin to [`PollWatcher`] rather
    /// than the OS-native backend (`FSEvents` on macOS) because:
    ///
    /// 1. The native backend's stream-creation latency is multi-second
    ///    on macOS in some test contexts, blocking [`FileQueue::open`]
    ///    long enough to be unusable.
    /// 2. The dequeue loop already polls at [`POLL_INTERVAL`] for the
    ///    cross-process case, so the watcher is a latency optimisation,
    ///    not a correctness requirement. `PollWatcher`'s polling
    ///    interleaves with our own with no correctness impact.
    /// 3. Same-process producers wake consumers via the in-process
    ///    [`Notify`] directly, bypassing the watcher entirely.
    _watcher: PollWatcher,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("root", &self.root)
            .field("pending", &self.pending)
            .field("tmp", &self.tmp)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl FileQueue {
    /// Open (or create) a [`FileQueue`] rooted at `path`. The root
    /// directory and its `pending/` and `tmp/` children are created if
    /// missing, and any orphan files left over in `tmp/` from a previous
    /// crash are removed before the queue starts serving requests.
    ///
    /// # Errors
    ///
    /// Returns [`FileQueueError::Io`] if the queue directory cannot be
    /// created or swept, or [`FileQueueError::Watcher`] if the underlying
    /// `notify` watcher cannot attach to `pending/`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FileQueueError> {
        let root = path.as_ref().to_path_buf();
        let pending = root.join(PENDING_DIR);
        let tmp = root.join(TMP_DIR);
        fs::create_dir_all(&pending)?;
        fs::create_dir_all(&tmp)?;

        // Sweep stale partial-writes and orphan claims left by previous
        // crashes. Anything in `tmp/` is by definition transient state.
        sweep_tmp(&tmp)?;

        let notify = Arc::new(Notify::new());
        let notify_for_watcher = Arc::clone(&notify);
        let watcher_config = NotifyConfig::default().with_poll_interval(POLL_INTERVAL);
        let mut watcher = PollWatcher::new(
            move |res: notify::Result<notify::Event>| {
                // We don't care about the event details — any change in
                // `pending/` is a hint that a new signal *might* be
                // available. The dequeue loop tolerates spurious wakes.
                // Errors from the watcher are intentionally swallowed
                // here; the poll-interval fallback in `dequeue` keeps
                // forward progress even if the watcher silently dies.
                if res.is_ok() {
                    notify_for_watcher.notify_one();
                }
            },
            watcher_config,
        )?;
        watcher.watch(&pending, RecursiveMode::NonRecursive)?;

        Ok(Self {
            inner: Arc::new(Inner {
                root,
                pending,
                tmp,
                next_seq: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                notify,
                _watcher: watcher,
            }),
        })
    }

    /// Root directory this queue lives under.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.root
    }

    /// Number of signals currently waiting in `pending/`. Intended for
    /// tests and observability.
    ///
    /// # Errors
    ///
    /// Returns [`FileQueueError::Io`] if the pending directory cannot be
    /// listed.
    pub async fn len(&self) -> Result<usize, FileQueueError> {
        let mut entries = tokio::fs::read_dir(&self.inner.pending).await?;
        let mut count = 0usize;
        while let Some(_entry) = entries.next_entry().await? {
            count += 1;
        }
        Ok(count)
    }

    /// `true` when no signals are currently waiting in `pending/`.
    ///
    /// # Errors
    ///
    /// Returns [`FileQueueError::Io`] on underlying errors.
    pub async fn is_empty(&self) -> Result<bool, FileQueueError> {
        Ok(self.len().await? == 0)
    }

    /// Try to claim and decode the lex-smallest entry in `pending/`. The
    /// outer loop retries when another consumer wins the rename race so
    /// that callers never observe a transient `NotFound`.
    async fn try_pop(&self) -> Result<Option<Signal>, FileQueueError> {
        loop {
            let Some(name) = smallest_pending(&self.inner.pending).await? else {
                return Ok(None);
            };

            let claim_name = format!(
                "{}{}{}-{}",
                name,
                CLAIM_INFIX,
                std::process::id(),
                self.inner.next_seq.fetch_add(1, Ordering::Relaxed)
            );
            let claim_path = self.inner.tmp.join(&claim_name);
            let pending_path = self.inner.pending.join(&name);

            match tokio::fs::rename(&pending_path, &claim_path).await {
                Ok(()) => {
                    let bytes = tokio::fs::read(&claim_path).await?;
                    let signal: Signal = serde_json::from_slice(&bytes)?;
                    tokio::fs::remove_file(&claim_path).await?;
                    return Ok(Some(signal));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Lost the rename race to another consumer. Loop and
                    // pick a fresh candidate.
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl FileQueue {
    async fn enqueue_signal(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> Result<(), FileQueueError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(FileQueueError::Closed);
        }

        let payload = serde_json::to_vec(&signal)?;

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        // Invert priority so that higher priorities sort first under the
        // plain lex order of `read_dir`.
        let priority_inv = u16::from(u8::MAX) - u16::from(priority.value());
        let signal_id = signal.id().to_string();
        let final_name = format!("{priority_inv:03}-{nanos:020}-{seq:010}-{signal_id}.json");
        let partial_name = format!("{final_name}{PARTIAL_SUFFIX}");

        let partial_path = self.inner.tmp.join(&partial_name);
        let pending_path = self.inner.pending.join(&final_name);

        // 1. Stage the payload in tmp/.
        tokio::fs::write(&partial_path, &payload).await?;

        // 2. fsync the partial file so the bytes are durable before the
        //    rename publishes the signal.
        let f = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&partial_path)
            .await?;
        f.sync_all().await?;
        drop(f);

        // 3. Atomic publish into pending/.
        tokio::fs::rename(&partial_path, &pending_path).await?;

        // 4. fsync the pending directory so the rename itself is durable
        //    (POSIX requires fsync of the containing directory to commit
        //    a rename to disk). On non-Unix platforms this step is
        //    skipped — those targets fall back to the OS's own
        //    eventually-consistent flushing behavior.
        #[cfg(unix)]
        {
            let dir = tokio::fs::OpenOptions::new()
                .read(true)
                .open(&self.inner.pending)
                .await?;
            dir.sync_all().await?;
        }

        // 5. Wake any in-process consumer parked on the notifier. The
        //    cross-process wake is delivered by the filesystem watcher
        //    callback, which calls `notify_one` on the same shared
        //    `Notify`.
        self.inner.notify.notify_one();
        Ok(())
    }

    async fn dequeue_signal(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Signal>, FileQueueError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if let Some(signal) = self.try_pop().await? {
                return Ok(Some(signal));
            }
            if self.inner.closed.load(Ordering::SeqCst) {
                // Recheck for any signal that landed between the
                // try_pop and the closed observation, otherwise return
                // the drained terminal state. Mirrors the closed-recheck
                // pattern in `InMemoryQueue::dequeue`.
                if let Some(signal) = self.try_pop().await? {
                    return Ok(Some(signal));
                }
                return Ok(None);
            }

            // Arm the notifier *before* the recheck so a concurrent
            // producer either fills `pending/` (we'll see it) or fires
            // notify_one after we've registered (we'll be woken).
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if let Some(signal) = self.try_pop().await? {
                return Ok(Some(signal));
            }
            if self.inner.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }

            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                () = &mut notified => {}
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

}

#[async_trait]
impl Queue for FileQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.enqueue_signal(signal, priority)
            .await
            .map_err(QueueError::new)
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        self.dequeue_signal(cancel).await.map_err(QueueError::new)
    }

    async fn close(&self) -> Result<(), QueueError> {
        // Idempotent: a second close is a no-op. Use SeqCst to pair with
        // the dequeue/enqueue checks; the queue is not on a hot enough
        // path to justify weaker orderings.
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        // Wake every currently-parked dequeue so they observe the
        // closed flag and return `Ok(None)` once `pending/` is empty.
        self.inner.notify.notify_waiters();
        Ok(())
    }
}

/// Remove every file in `tmp/`. Anything that survives a process exit
/// here is orphan state — either a `.partial` from a crashed enqueue or
/// a `.claim-*` from a crashed dequeue.
fn sweep_tmp(tmp: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(tmp)? {
        let entry = entry?;
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            // Tolerate a concurrent sweeper or a vanished entry. We
            // explicitly do not recurse into nested directories: the
            // queue layout never creates any.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Return the lex-smallest entry name in `pending/`, or `None` when the
/// directory is empty. We avoid sorting the entire listing because the
/// queue only ever needs the next single entry; tracking the running
/// minimum is O(n) instead of O(n log n).
async fn smallest_pending(pending: &Path) -> Result<Option<String>, FileQueueError> {
    let mut entries = tokio::fs::read_dir(pending).await?;
    let mut min: Option<String> = None;
    while let Some(entry) = entries.next_entry().await? {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        min = Some(match min {
            Some(curr) if curr.as_str() <= name.as_str() => curr,
            _ => name,
        });
    }
    Ok(min)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{Metadata, MetadataKey, MetadataValue, Priority, Queue, Signal};
    use tempfile::tempdir;
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn signal_with(label: &str) -> Signal {
        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("label").expect("valid key"),
            MetadataValue::String(label.into()),
        );
        Signal::new(metadata)
    }

    fn label_of(signal: &Signal) -> String {
        match signal
            .metadata()
            .get(&MetadataKey::new("label").expect("valid key"))
            .expect("label present")
        {
            MetadataValue::String(s) => s.clone(),
            other => panic!("unexpected metadata variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("queue");

        {
            let queue = FileQueue::open(&root).expect("open");
            queue
                .enqueue(signal_with("persistent"), Priority::HIGH)
                .await
                .expect("queue");
        }

        let reopened = FileQueue::open(&root).expect("reopen");
        let cancel = CancellationToken::new();
        let signal = reopened
            .dequeue(cancel)
            .await
            .expect("dequeue ok")
            .expect("some");
        assert_eq!(label_of(&signal), "persistent");
        assert!(reopened.is_empty().await.expect("len ok"));
    }

    #[tokio::test]
    async fn priority_ordering() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");

        queue
            .enqueue(signal_with("low"), Priority::LOW)
            .await
            .expect("queue");
        queue
            .enqueue(signal_with("critical"), Priority::CRITICAL)
            .await
            .expect("queue");
        queue
            .enqueue(signal_with("normal"), Priority::NORMAL)
            .await
            .expect("queue");
        queue
            .enqueue(signal_with("high"), Priority::HIGH)
            .await
            .expect("queue");

        let cancel = CancellationToken::new();
        let mut order = Vec::new();
        for _ in 0..4 {
            let s = queue
                .dequeue(cancel.clone())
                .await
                .expect("dequeue ok")
                .expect("some");
            order.push(label_of(&s));
        }
        assert_eq!(order, vec!["critical", "high", "normal", "low"]);
    }

    #[tokio::test]
    async fn cancel_on_parked_dequeue() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");

        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            queue_clone
                .dequeue(cancel_for_task)
                .await
                .expect("dequeue ok")
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        let result = timeout(Duration::from_secs(1), handle)
            .await
            .expect("not timed out")
            .expect("join ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn two_handles_share_same_directory() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("queue");

        let producer = FileQueue::open(&root).expect("open producer");
        let consumer = FileQueue::open(&root).expect("open consumer");

        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let consumer_task =
            tokio::spawn(
                async move { consumer.dequeue(cancel_for_task).await.expect("dequeue ok") },
            );

        // Small delay so the consumer has time to land in its poll loop.
        tokio::time::sleep(Duration::from_millis(20)).await;
        producer
            .enqueue(signal_with("cross"), Priority::NORMAL)
            .await
            .expect("queue");

        let signal = timeout(Duration::from_secs(2), consumer_task)
            .await
            .expect("not timed out")
            .expect("join ok")
            .expect("some");
        assert_eq!(label_of(&signal), "cross");
    }

    #[tokio::test]
    async fn fifo_within_single_priority() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");

        for i in 0..5 {
            queue
                .enqueue(signal_with(&format!("s{i}")), Priority::NORMAL)
                .await
                .expect("queue");
        }

        let cancel = CancellationToken::new();
        for i in 0..5 {
            let s = queue
                .dequeue(cancel.clone())
                .await
                .expect("dequeue ok")
                .expect("some");
            assert_eq!(label_of(&s), format!("s{i}"));
        }
    }

    #[tokio::test]
    async fn enqueue_lands_in_pending_directory() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("queue");
        let queue = FileQueue::open(&root).expect("open");

        queue
            .enqueue(signal_with("only"), Priority::NORMAL)
            .await
            .expect("queue");

        let pending = fs::read_dir(root.join(PENDING_DIR))
            .expect("pending readable")
            .map(|e| e.expect("entry"))
            .collect::<Vec<_>>();
        let tmp = fs::read_dir(root.join(TMP_DIR))
            .expect("tmp readable")
            .map(|e| e.expect("entry"))
            .collect::<Vec<_>>();

        assert_eq!(pending.len(), 1, "exactly one signal in pending/");
        assert!(tmp.is_empty(), "tmp/ must be empty after enqueue");
    }

    #[tokio::test]
    async fn crash_orphans_swept_on_open() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("queue");
        fs::create_dir_all(root.join(TMP_DIR)).expect("create tmp");
        fs::create_dir_all(root.join(PENDING_DIR)).expect("create pending");

        // Plant a leftover partial-write and a leftover claim that look
        // exactly like what a crashed enqueue / dequeue would leave.
        let partial = root.join(TMP_DIR).join("050-junk.json.partial");
        fs::write(&partial, b"junk").expect("write partial");
        let claim = root.join(TMP_DIR).join("050-junk.json.claim-1-1");
        fs::write(&claim, b"junk").expect("write claim");

        let _queue = FileQueue::open(&root).expect("open");
        assert!(!partial.exists(), "partial orphan must be swept");
        assert!(!claim.exists(), "claim orphan must be swept");
    }

    #[tokio::test]
    async fn arbitrary_priority_value() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");

        // Use values outside the four named constants to confirm the
        // filename-prefix scheme covers the full u8 range, not just the
        // canonical buckets.
        queue
            .enqueue(signal_with("p13"), Priority::new(13))
            .await
            .expect("queue 13");
        queue
            .enqueue(signal_with("p200"), Priority::new(200))
            .await
            .expect("queue 200");

        let cancel = CancellationToken::new();
        let first = queue
            .dequeue(cancel.clone())
            .await
            .expect("dequeue ok")
            .expect("some");
        let second = queue
            .dequeue(cancel)
            .await
            .expect("dequeue ok")
            .expect("some");
        assert_eq!(label_of(&first), "p200");
        assert_eq!(label_of(&second), "p13");
    }

    #[tokio::test]
    async fn close_then_dequeue_returns_drained() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");

        queue
            .enqueue(signal_with("last"), Priority::NORMAL)
            .await
            .expect("queue");
        queue.close().await.expect("close");

        // Enqueue after close must be rejected.
        let err = queue
            .enqueue(signal_with("after-close"), Priority::NORMAL)
            .await
            .expect_err("post-close enqueue rejected");
        assert!(matches!(
            err.downcast_ref::<FileQueueError>(),
            Some(FileQueueError::Closed)
        ));

        let cancel = CancellationToken::new();
        let drained = queue
            .dequeue(cancel.clone())
            .await
            .expect("dequeue ok")
            .expect("queued signal still drains after close");
        assert_eq!(label_of(&drained), "last");

        let after = queue.dequeue(cancel).await.expect("dequeue ok");
        assert!(after.is_none(), "drained queue returns Ok(None)");
    }

    #[tokio::test]
    async fn double_close_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let queue = FileQueue::open(dir.path().join("queue")).expect("open");
        queue.close().await.expect("first close");
        queue.close().await.expect("second close is a no-op");
    }
}
