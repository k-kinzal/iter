//! In-memory [`Queue`] implementation.
//!
//! [`InMemoryQueue`] keeps signals in a `BinaryHeap` ordered by
//! [`Priority`] (descending) and FIFO within a priority. It is the default
//! queue used when no persistence or distribution is required.

pub mod error;

pub use error::InMemoryQueueError;

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::SystemTime;

use crate::queue::QueueError;
use crate::{Priority, Queue, Signal};
use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

/// A heap entry pairing a [`Signal`] with its ordering key.
///
/// Ordering is `(Priority desc, enqueue time asc)` so that higher priority
/// signals — and within the same priority, the oldest signals — come out
/// first when the entry is stored in a max-heap.
#[derive(Debug)]
struct Entry {
    priority: Priority,
    /// Wrapped in `Reverse` so that *earlier* timestamps compare *greater*
    /// inside the max-heap, yielding FIFO order within a priority bucket.
    enqueued_at: Reverse<SystemTime>,
    /// Monotonic insertion sequence used as a final tiebreaker. Two signals
    /// captured within the same `SystemTime` tick must still dequeue in
    /// insertion order; the `seq` guarantees this regardless of clock
    /// resolution.
    seq: Reverse<u64>,
    signal: Signal,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Primary: higher priority first (max-heap on priority).
        self.priority
            .cmp(&other.priority)
            // Secondary: earlier `enqueued_at` first (Reverse converts the
            // natural "later is greater" ordering into "earlier is greater").
            .then_with(|| self.enqueued_at.cmp(&other.enqueued_at))
            // Tertiary: smaller insertion sequence first.
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

#[derive(Debug, Default)]
struct State {
    heap: BinaryHeap<Entry>,
    next_seq: u64,
    /// Set by [`Queue::close`]. Once `true`, further enqueues are rejected
    /// with [`InMemoryQueueError::Closed`] and a drained heap makes
    /// `dequeue` return `Ok(None)`.
    closed: bool,
}

/// A non-persistent in-memory priority queue.
///
/// Cheap to clone via the inner [`Arc`] — every clone shares the same
/// underlying heap. The queue is `Send + Sync` and is the default
/// [`Queue`](crate::Queue) implementation when no persistence or
/// distribution is required.
///
/// # Example
///
/// ```no_run
/// use iter_core::{Metadata, Priority, Queue, Signal};
/// use iter_core::queue::InMemoryQueue;
/// use tokio_util::sync::CancellationToken;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let queue = InMemoryQueue::new();
/// queue.enqueue(Signal::new(Metadata::new()), Priority::HIGH).await?;
/// let signal = queue.dequeue(CancellationToken::new()).await?;
/// assert!(signal.is_some());
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Default)]
pub struct InMemoryQueue {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    state: Mutex<State>,
    notify: Notify,
}

impl InMemoryQueue {
    /// Create an empty in-memory queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of signals currently buffered.
    ///
    /// Primarily intended for tests and observability.
    pub async fn len(&self) -> usize {
        self.inner.state.lock().await.heap.len()
    }

    /// Returns `true` if the queue currently holds no signals.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

#[async_trait]
impl Queue for InMemoryQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        {
            let mut state = self.inner.state.lock().await;
            if state.closed {
                return Err(QueueError::new(InMemoryQueueError::Closed));
            }
            let seq = state.next_seq;
            state.next_seq = state.next_seq.wrapping_add(1);
            state.heap.push(Entry {
                priority,
                enqueued_at: Reverse(SystemTime::now()),
                seq: Reverse(seq),
                signal,
            });
        }
        // Wake exactly one parked dequeue. If there are no waiters the
        // permit is stored and consumed by the next `notified().await`.
        self.inner.notify.notify_one();
        Ok(())
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        loop {
            // Fast path: pop under the lock if anything is queued, or
            // observe the drained-and-closed terminal state.
            {
                let mut state = self.inner.state.lock().await;
                if let Some(entry) = state.heap.pop() {
                    return Ok(Some(entry.signal));
                }
                if state.closed {
                    return Ok(None);
                }
            }

            // Slow path: register interest *before* re-checking the heap so
            // that any concurrent `queue` either fills the heap (we'll see
            // it on the recheck) or fires `notify_one` after we've armed the
            // future (we'll be woken). The same recheck handles `close`:
            // the `notify_waiters` call inside `close` wakes us, we loop
            // back around, observe `closed` on the fast path, and return
            // `Ok(None)`.
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // Recheck after arming, in case a producer or closer raced
            // ahead of the arm. Without this we could miss a wake.
            {
                let mut state = self.inner.state.lock().await;
                if let Some(entry) = state.heap.pop() {
                    return Ok(Some(entry.signal));
                }
                if state.closed {
                    return Ok(None);
                }
            }

            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                () = notified => {}
            }
        }
    }

    async fn close(&self) -> Result<(), QueueError> {
        {
            let mut state = self.inner.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
        }
        // Wake every currently-parked `dequeue` so they observe the
        // closed flag and return `Ok(None)` when the heap is empty. New
        // dequeue calls after this point see `closed = true` on the fast
        // path without needing a wake.
        self.inner.notify.notify_waiters();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{Metadata, MetadataKey, MetadataValue, Priority, Queue, Signal};
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
    async fn fifo_within_single_priority() {
        let queue = InMemoryQueue::new();
        for i in 0..5 {
            queue
                .enqueue(signal_with(&format!("s{i}")), Priority::NORMAL)
                .await
                .expect("queue ok");
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
    async fn priority_ordering_critical_high_normal_low() {
        let queue = InMemoryQueue::new();
        // Insert in reverse priority order to ensure ordering is by priority
        // and not by insertion order.
        queue
            .enqueue(signal_with("low"), Priority::LOW)
            .await
            .expect("queue ok");
        queue
            .enqueue(signal_with("normal"), Priority::NORMAL)
            .await
            .expect("queue ok");
        queue
            .enqueue(signal_with("high"), Priority::HIGH)
            .await
            .expect("queue ok");
        queue
            .enqueue(signal_with("critical"), Priority::CRITICAL)
            .await
            .expect("queue ok");

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
    async fn dequeue_parks_until_signal_arrives() {
        let queue = InMemoryQueue::new();
        let cancel = CancellationToken::new();
        let queue_clone = queue.clone();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            queue_clone
                .dequeue(cancel_clone)
                .await
                .expect("dequeue ok")
                .expect("some")
        });

        // Give the dequeue task a moment to park.
        tokio::time::sleep(Duration::from_millis(20)).await;
        queue
            .enqueue(signal_with("late"), Priority::NORMAL)
            .await
            .expect("queue ok");

        let signal = timeout(Duration::from_secs(1), handle)
            .await
            .expect("not timed out")
            .expect("join ok");
        assert_eq!(label_of(&signal), "late");
    }

    #[tokio::test]
    async fn cancel_token_wakes_parked_dequeue() {
        let queue = InMemoryQueue::new();
        let cancel = CancellationToken::new();

        let queue_clone = queue.clone();
        let cancel_clone = cancel.clone();
        let handle =
            tokio::spawn(async move { queue_clone.dequeue(cancel_clone).await.expect("ok") });

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();

        let result = timeout(Duration::from_secs(1), handle)
            .await
            .expect("not timed out")
            .expect("join ok");
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_producers_and_consumers() {
        let queue = InMemoryQueue::new();
        let total: usize = 100;
        let producers = 4usize;
        let consumers = 4usize;
        let per_producer = total / producers;

        let mut producer_handles = Vec::new();
        for p in 0..producers {
            let q = queue.clone();
            producer_handles.push(tokio::spawn(async move {
                for i in 0..per_producer {
                    q.enqueue(signal_with(&format!("p{p}-{i}")), Priority::NORMAL)
                        .await
                        .expect("queue ok");
                }
            }));
        }

        let cancel = CancellationToken::new();
        let mut consumer_handles = Vec::new();
        for _ in 0..consumers {
            let q = queue.clone();
            let c = cancel.clone();
            consumer_handles.push(tokio::spawn(async move {
                let mut collected = Vec::new();
                while let Some(s) = q.dequeue(c.clone()).await.expect("dequeue ok") {
                    collected.push(label_of(&s));
                }
                collected
            }));
        }

        for h in producer_handles {
            h.await.expect("producer join");
        }

        // Wait until the queue has drained, then cancel so consumers exit.
        loop {
            if queue.is_empty().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();

        let mut all = Vec::new();
        for h in consumer_handles {
            all.extend(h.await.expect("consumer join"));
        }
        all.sort();
        assert_eq!(all.len(), total);

        let mut expected: Vec<String> = (0..producers)
            .flat_map(|p| (0..per_producer).map(move |i| format!("p{p}-{i}")))
            .collect();
        expected.sort();
        assert_eq!(all, expected);
    }

    #[tokio::test]
    async fn is_empty_queue_returns_on_cancel() {
        // Drop-test: a parked dequeue must respect cancellation when the
        // queue is empty rather than hang forever.
        let queue = InMemoryQueue::new();
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let handle =
            tokio::spawn(async move { queue.dequeue(cancel_for_task).await.expect("dequeue ok") });
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
        let res = timeout(Duration::from_secs(1), handle)
            .await
            .expect("not timed out")
            .expect("join ok");
        assert!(res.is_none());
    }
}
