//! [`BudgetedQueue`] — the Queue-side enforcement point for a Trigger's
//! **emission budget**.
//!
//! A Trigger may be told to publish at most N signals and then stop. The
//! budget is the Trigger's, but it is enforced *here*, at the boundary: this
//! decorator counts successful enqueues and, on reaching the budget, fires a
//! [`CancellationToken`] so the Trigger drains and the queue can close. A
//! budget of `0` disables the cancellation, so wrapping is cheap and uniform
//! regardless of whether a budget was set.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::queue::{Priority, Queue, QueueError};
use crate::signal::Signal;

/// Decorator that counts successful enqueues and triggers a
/// [`CancellationToken`] once the count reaches the emission budget `max`.
#[derive(Clone)]
pub struct BudgetedQueue {
    inner: Arc<dyn Queue>,
    count: Arc<AtomicU64>,
    max: u64,
    cancel: CancellationToken,
}

impl BudgetedQueue {
    /// Wrap `inner` with a counter that fires `cancel` once `max` signals have
    /// been published. `max == 0` means unlimited.
    #[must_use]
    pub fn new(inner: Arc<dyn Queue>, max: u64, cancel: CancellationToken) -> Self {
        Self {
            inner,
            count: Arc::new(AtomicU64::new(0)),
            max,
            cancel,
        }
    }

    /// Number of signals successfully published through this decorator.
    #[must_use]
    pub fn published(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Queue for BudgetedQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.inner.enqueue(signal, priority).await?;
        let new_count = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.max > 0 && new_count >= self.max {
            info!(
                emitted = new_count,
                max = self.max,
                "emission budget reached, requesting shutdown"
            );
            self.cancel.cancel();
        }
        Ok(())
    }

    async fn dequeue(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Signal>, QueueError> {
        self.inner.dequeue(cancel).await
    }

    async fn close(&self) -> Result<(), QueueError> {
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::InMemoryQueue;
    use crate::signal::Metadata;

    #[tokio::test]
    async fn counts_and_cancels_at_budget() {
        let inner: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
        let token = CancellationToken::new();
        let budgeted = BudgetedQueue::new(inner, 2, token.clone());

        budgeted
            .enqueue(Signal::new(Metadata::new()), Priority::NORMAL)
            .await
            .expect("first");
        assert!(!token.is_cancelled());
        budgeted
            .enqueue(Signal::new(Metadata::new()), Priority::NORMAL)
            .await
            .expect("second");
        assert!(token.is_cancelled(), "token must fire at budget");
        assert_eq!(budgeted.published(), 2);
    }

    #[tokio::test]
    async fn zero_budget_means_unlimited() {
        let inner: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
        let token = CancellationToken::new();
        let budgeted = BudgetedQueue::new(inner, 0, token.clone());
        for _ in 0..50 {
            budgeted
                .enqueue(Signal::new(Metadata::new()), Priority::NORMAL)
                .await
                .expect("enqueue");
        }
        assert!(!token.is_cancelled(), "0 must not cancel");
        assert_eq!(budgeted.published(), 50);
    }
}
