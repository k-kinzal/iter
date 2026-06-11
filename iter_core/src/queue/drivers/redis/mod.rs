//! Redis-backed distributed [`Queue`] implementation.
//!
//! [`RedisQueue`] stores signals in a single Redis sorted set (`ZSET`) keyed
//! by a caller-supplied key. The score encodes both priority and enqueue
//! time so that `ZPOPMIN`/`BZPOPMIN` always returns the right signal next:
//!
//! ```text
//! score = -(priority as f64) * 1e15 - created_at_nanos
//! ```
//!
//! Lower scores pop first, so negating priority puts the highest priority
//! at the front; subtracting the nanosecond timestamp breaks ties in FIFO
//! order.

pub mod error;

pub use error::RedisQueueError;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::queue::QueueError;
use crate::{Priority, Queue, Signal};
use async_trait::async_trait;
use redis::{AsyncCommands, Client};
use tokio_util::sync::CancellationToken;

/// Multiplier applied to priority when building the score. Chosen so that
/// any signed-64-bit wall-clock nanosecond value (which is far less than
/// 1e15 — about 1e18 ns ≈ 2262 years, but the distance between two
/// priorities still dominates any realistic queue age) cannot flip the
/// priority ordering.
/// Constant chosen so a one-tick priority diff dominates ~30 years of
/// elapsed-seconds tie-breaker (`as_secs_f64()` ranges around `1.7e9`).
const PRIORITY_SCALE: f64 = 1e10;

/// BZPOPMIN blocking timeout (seconds). Short enough to observe a
/// cancellation promptly, long enough to avoid hammering Redis.
const BLOCK_TIMEOUT_SECS: f64 = 1.0;

/// Distributed priority queue backed by a Redis sorted set.
///
/// A fresh asynchronous connection is obtained for every operation via
/// [`Client::get_multiplexed_async_connection`]. The multiplexed connection
/// is the current (non-deprecated) safe way to share a Redis connection
/// across concurrent async callers; acquiring one per operation keeps the
/// queue simple and side-steps ownership questions entirely.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::{Metadata, Priority, Queue, Signal};
/// use iter_core::queue::RedisQueue;
///
/// let queue = RedisQueue::connect("redis://127.0.0.1/", "iter:queue").await?;
/// queue.enqueue(Signal::new(Metadata::new()), Priority::HIGH).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct RedisQueue {
    client: Client,
    key: String,
}

impl RedisQueue {
    /// Connect to a Redis instance and prepare a queue bound to `key`.
    ///
    /// Performs one round-trip (a `PING`) to verify the connection before
    /// returning.
    ///
    /// # Errors
    ///
    /// Returns [`RedisQueueError::Redis`] if the URL cannot be parsed or
    /// the initial connection check fails.
    pub async fn connect(url: &str, key: impl Into<String>) -> Result<Self, RedisQueueError> {
        let client = Client::open(url)?;
        // Eager round-trip so caller mistakes (bad URL, auth, etc.) surface
        // immediately rather than on the first enqueue.
        let mut conn = client.get_multiplexed_async_connection().await?;
        redis::cmd("PING").query_async::<()>(&mut conn).await?;
        Ok(Self {
            client,
            key: key.into(),
        })
    }

    /// Redis key this queue is bound to.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Delete the backing sorted set. Primarily intended for tests.
    ///
    /// # Errors
    ///
    /// Returns [`RedisQueueError::Redis`] on underlying errors.
    pub async fn clear(&self) -> Result<(), RedisQueueError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let _: () = conn.del(&self.key).await?;
        Ok(())
    }

    /// Number of signals currently waiting in the sorted set.
    ///
    /// # Errors
    ///
    /// Returns [`RedisQueueError::Redis`] on underlying errors.
    pub async fn len(&self) -> Result<usize, RedisQueueError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let n: usize = conn.zcard(&self.key).await?;
        Ok(n)
    }

    /// `true` when the queue holds no signals.
    ///
    /// # Errors
    ///
    /// Returns [`RedisQueueError::Redis`] on underlying errors.
    pub async fn is_empty(&self) -> Result<bool, RedisQueueError> {
        Ok(self.len().await? == 0)
    }
}

/// Build the score for a signal based on its priority and current wall
/// clock. Lower scores pop first, so higher priorities map to more
/// negative values. `Duration::as_secs_f64` already produces an f64; the
/// loss of fractional-nanosecond precision is acceptable for queue ordering.
fn score_for(priority: Priority) -> f64 {
    let secs_f = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    -f64::from(priority.value()) * PRIORITY_SCALE - secs_f
}

/// The member written to the sorted set. Uniqueness is guaranteed by the
/// embedded [`SignalId`](crate::SignalId) (UUID v7). The payload is
/// encoded as JSON so it round-trips through `BZPOPMIN` as a single string.
fn encode_member(signal: &Signal) -> Result<String, RedisQueueError> {
    Ok(serde_json::to_string(signal)?)
}

fn decode_member(member: &str) -> Result<Signal, RedisQueueError> {
    Ok(serde_json::from_str(member)?)
}

impl RedisQueue {
    async fn enqueue_signal(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> Result<(), RedisQueueError> {
        let member = encode_member(&signal)?;
        let score = score_for(priority);
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let _: () = conn.zadd(&self.key, member, score).await?;
        Ok(())
    }

    async fn dequeue_signal(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Signal>, RedisQueueError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }

            let mut conn = self.client.get_multiplexed_async_connection().await?;

            let bzpop =
                conn.bzpopmin::<_, Option<(String, String, String)>>(&self.key, BLOCK_TIMEOUT_SECS);

            let result = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                r = bzpop => r,
            };

            // Timed out (None) — loop and re-check cancel.
            if let Some((_key, member, _score)) = result? {
                return Ok(Some(decode_member(&member)?));
            }
        }
    }
}

#[async_trait]
impl Queue for RedisQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.enqueue_signal(signal, priority)
            .await
            .map_err(QueueError::new)
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        self.dequeue_signal(cancel).await.map_err(QueueError::new)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{Metadata, MetadataKey, MetadataValue, Priority, Queue, Signal};
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// Env var that must be set for the tests that actually connect. When
    /// absent we skip (return early) rather than fail.
    const ENV_URL: &str = "ITER_REDIS_TEST_URL";

    fn test_url() -> Option<String> {
        std::env::var(ENV_URL).ok()
    }

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

    fn test_key(suffix: &str) -> String {
        format!("iter_queue::test::{}::{}", std::process::id(), suffix)
    }

    #[test]
    fn score_higher_priority_is_lower_score() {
        // Sanity-check the score function: the queue is ordered by
        // increasing score, so higher priorities must produce smaller
        // (more negative) scores.
        let high = score_for(Priority::CRITICAL);
        let low = score_for(Priority::LOW);
        assert!(
            high < low,
            "expected CRITICAL score {high} < LOW score {low}"
        );
    }

    #[tokio::test]
    #[ignore = "requires running Redis via ITER_REDIS_TEST_URL"]
    async fn priority_ordering() {
        let Some(url) = test_url() else {
            tracing::info!("skipping: {ENV_URL} not set");
            return;
        };
        let queue = RedisQueue::connect(&url, test_key("priority_ordering"))
            .await
            .expect("connect");
        queue.clear().await.expect("clear");

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
        queue.clear().await.expect("cleanup");
    }

    #[tokio::test]
    #[ignore = "requires running Redis via ITER_REDIS_TEST_URL"]
    async fn cancel_on_parked_dequeue() {
        let Some(url) = test_url() else {
            tracing::info!("skipping: {ENV_URL} not set");
            return;
        };
        let queue = RedisQueue::connect(&url, test_key("cancel_parked"))
            .await
            .expect("connect");
        queue.clear().await.expect("clear");

        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            queue_clone
                .dequeue(cancel_for_task)
                .await
                .expect("dequeue ok")
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        let result = timeout(Duration::from_secs(3), handle)
            .await
            .expect("not timed out")
            .expect("join ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore = "requires running Redis via ITER_REDIS_TEST_URL"]
    async fn queue_and_dequeue_roundtrip() {
        let Some(url) = test_url() else {
            tracing::info!("skipping: {ENV_URL} not set");
            return;
        };
        let queue = RedisQueue::connect(&url, test_key("roundtrip"))
            .await
            .expect("connect");
        queue.clear().await.expect("clear");

        queue
            .enqueue(signal_with("one"), Priority::NORMAL)
            .await
            .expect("queue");
        let cancel = CancellationToken::new();
        let signal = queue
            .dequeue(cancel)
            .await
            .expect("dequeue ok")
            .expect("some");
        assert_eq!(label_of(&signal), "one");
        queue.clear().await.expect("cleanup");
    }
}
