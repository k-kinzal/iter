//! Wrapper [`Queue`] that counts published signals and cancels a token
//! when a configured threshold is reached.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iter_core::{Priority, Queue, Signal};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Decorator that counts successful [`Queue::queue`] calls and triggers
/// a [`CancellationToken`] when the count reaches `max`.
///
/// `max == 0` disables the cancellation behaviour, so wrapping is cheap
/// and uniform regardless of whether the caller passed `--max-signals`.
#[derive(Debug, Clone)]
pub struct CountingQueue<Q: Queue> {
    inner: Arc<Q>,
    count: Arc<AtomicU64>,
    max: u64,
    cancel: CancellationToken,
}

impl<Q: Queue> CountingQueue<Q> {
    /// Wrap `inner` with a counter that fires `cancel` once `max` signals
    /// have been published.
    #[must_use]
    pub fn new(inner: Arc<Q>, max: u64, cancel: CancellationToken) -> Self {
        Self {
            inner,
            count: Arc::new(AtomicU64::new(0)),
            max,
            cancel,
        }
    }

    /// Borrow the wrapped inner queue.
    #[must_use]
    pub fn inner(&self) -> &Arc<Q> {
        &self.inner
    }

    /// Number of signals successfully published through this wrapper.
    #[must_use]
    pub fn published(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

impl<Q: Queue + 'static> Queue for CountingQueue<Q> {
    type Error = Q::Error;

    fn queue(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let inner = self.inner.clone();
        let count = self.count.clone();
        let max = self.max;
        let cancel = self.cancel.clone();
        async move {
            inner.queue(signal, priority).await?;
            let new_count = count.fetch_add(1, Ordering::Relaxed) + 1;
            if max > 0 && new_count >= max {
                info!(
                    emitted = new_count,
                    max, "max-signals reached, requesting shutdown"
                );
                cancel.cancel();
            }
            Ok(())
        }
    }

    fn dequeue(
        &self,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<Option<Signal>, Self::Error>> + Send {
        let inner = self.inner.clone();
        async move { inner.dequeue(cancel).await }
    }

    fn close(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let inner = self.inner.clone();
        async move { inner.close().await }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::queue::InMemoryQueue;
    use iter_core::{Metadata, Signal};

    #[tokio::test]
    async fn counts_and_cancels_at_threshold() {
        let inner = Arc::new(InMemoryQueue::new());
        let token = CancellationToken::new();
        let wrapper = CountingQueue::new(inner.clone(), 2, token.clone());

        wrapper
            .queue(Signal::new(Metadata::new()), Priority::NORMAL)
            .await
            .expect("first");
        assert!(!token.is_cancelled());
        wrapper
            .queue(Signal::new(Metadata::new()), Priority::NORMAL)
            .await
            .expect("second");
        assert!(token.is_cancelled(), "token must fire at threshold");
        assert_eq!(wrapper.published(), 2);
    }

    #[tokio::test]
    async fn zero_max_means_unlimited() {
        let inner = Arc::new(InMemoryQueue::new());
        let token = CancellationToken::new();
        let wrapper = CountingQueue::new(inner.clone(), 0, token.clone());
        for _ in 0..50 {
            wrapper
                .queue(Signal::new(Metadata::new()), Priority::NORMAL)
                .await
                .expect("queue");
        }
        assert!(!token.is_cancelled(), "0 must not cancel");
        assert_eq!(wrapper.published(), 50);
    }
}
