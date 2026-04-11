//! [`Trigger`] — the signal emitter that trigger CLIs use to push events
//! into an iter queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iter_core::queue::{Priority, Queue};
use iter_core::signal::{Metadata, MetadataKey, MetadataValue, Signal};
use thiserror::Error;
use tracing::info;

/// Errors from [`Trigger::emit`].
#[derive(Debug, Error)]
pub enum EmitError<E: std::error::Error + 'static> {
    /// The underlying queue rejected the signal.
    #[error("queue error: {0}")]
    Queue(#[source] E),
}

/// Configuration for a [`Trigger`].
pub struct TriggerConfig {
    /// Priority assigned to every emitted signal.
    pub priority: Priority,
    /// Static metadata attached to every emitted signal.
    pub base_metadata: Vec<(MetadataKey, String)>,
    /// Maximum number of signals to emit before [`Trigger::should_stop`]
    /// returns `true`. Zero means unlimited.
    pub max_signals: u64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            priority: Priority::NORMAL,
            base_metadata: Vec::new(),
            max_signals: 0,
        }
    }
}

/// Per-emission metadata builder.
///
/// Each call to [`Trigger::emit`] takes a `TriggerEvent` that carries
/// event-specific metadata entries. These are merged with the base
/// metadata from [`TriggerConfig`].
pub struct TriggerEvent {
    extra: Vec<(MetadataKey, String)>,
}

impl TriggerEvent {
    /// Create an empty event.
    #[must_use]
    pub fn new() -> Self {
        Self { extra: Vec::new() }
    }

    /// Attach a metadata key-value pair to this event.
    ///
    /// # Panics
    ///
    /// Panics if `key` is not a valid [`MetadataKey`].
    #[must_use]
    pub fn meta(mut self, key: &str, value: impl Into<String>) -> Self {
        let key = MetadataKey::new(key).expect("valid metadata key");
        self.extra.push((key, value.into()));
        self
    }

    /// Attach a metadata key-value pair, accepting a pre-validated key.
    #[must_use]
    pub fn meta_key(mut self, key: MetadataKey, value: impl Into<String>) -> Self {
        self.extra.push((key, value.into()));
        self
    }
}

impl Default for TriggerEvent {
    fn default() -> Self {
        Self::new()
    }
}

/// The trigger emitter. Holds a queue handle and emits signals.
///
/// Created by a trigger CLI, this struct encapsulates the "emit a signal
/// into a queue" capability. It tracks the emission count internally and
/// provides [`should_stop`](Self::should_stop) for the CLI's loop to
/// check against the configured `max_signals`.
pub struct Trigger<Q: Queue> {
    queue: Arc<Q>,
    config: TriggerConfig,
    count: AtomicU64,
}

impl<Q: Queue> Trigger<Q> {
    /// Create a new trigger emitter.
    pub fn new(config: TriggerConfig, queue: Arc<Q>) -> Self {
        Self {
            queue,
            config,
            count: AtomicU64::new(0),
        }
    }

    /// Emit one signal into the queue.
    ///
    /// Builds a [`Signal`] from the base metadata in [`TriggerConfig`]
    /// merged with the per-event metadata in [`TriggerEvent`], then
    /// enqueues it at the configured priority.
    ///
    /// # Errors
    ///
    /// Returns [`EmitError`] if the queue rejects the signal.
    pub async fn emit(&self, event: TriggerEvent) -> Result<(), EmitError<Q::Error>> {
        let mut md = Metadata::new();
        for (k, v) in &self.config.base_metadata {
            md.insert(k.clone(), MetadataValue::String(v.clone()));
        }
        for (k, v) in event.extra {
            md.insert(k, MetadataValue::String(v));
        }
        let signal = Signal::new(md);
        self.queue
            .queue(signal, self.config.priority)
            .await
            .map_err(EmitError::Queue)?;
        let new_count = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.config.max_signals > 0 && new_count >= self.config.max_signals {
            info!(
                emitted = new_count,
                max = self.config.max_signals,
                "max-signals reached"
            );
        }
        Ok(())
    }

    /// Whether the trigger should stop emitting.
    ///
    /// Returns `true` when `max_signals > 0` and the emission count
    /// has reached the configured maximum.
    #[must_use]
    pub fn should_stop(&self) -> bool {
        let max = self.config.max_signals;
        max > 0 && self.count.load(Ordering::Relaxed) >= max
    }

    /// Number of signals successfully emitted.
    #[must_use]
    pub fn emitted(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Borrow the underlying queue.
    #[must_use]
    pub fn queue(&self) -> &Q {
        &self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::queue::InMemoryQueue;
    use std::sync::Arc;

    #[tokio::test]
    async fn emit_and_count() {
        let q = Arc::new(InMemoryQueue::new());
        let trigger = Trigger::new(
            TriggerConfig {
                max_signals: 2,
                ..TriggerConfig::default()
            },
            q.clone(),
        );

        trigger.emit(TriggerEvent::new()).await.unwrap();
        assert_eq!(trigger.emitted(), 1);
        assert!(!trigger.should_stop());

        trigger.emit(TriggerEvent::new()).await.unwrap();
        assert_eq!(trigger.emitted(), 2);
        assert!(trigger.should_stop());
    }

    #[tokio::test]
    async fn zero_max_means_unlimited() {
        let q = Arc::new(InMemoryQueue::new());
        let trigger = Trigger::new(TriggerConfig::default(), q);

        for _ in 0..50 {
            trigger.emit(TriggerEvent::new()).await.unwrap();
        }
        assert!(!trigger.should_stop());
    }

    #[tokio::test]
    async fn base_and_event_metadata_merge() {
        let q = Arc::new(InMemoryQueue::new());
        let trigger = Trigger::new(
            TriggerConfig {
                base_metadata: vec![(MetadataKey::new("source").unwrap(), "test".into())],
                ..TriggerConfig::default()
            },
            q.clone(),
        );

        trigger
            .emit(TriggerEvent::new().meta("event_key", "ev_val"))
            .await
            .unwrap();

        let signal = q
            .dequeue(tokio_util::sync::CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        let md = signal.metadata();
        assert_eq!(
            md.get(&MetadataKey::new("source").unwrap()),
            Some(&MetadataValue::String("test".into()))
        );
        assert_eq!(
            md.get(&MetadataKey::new("event_key").unwrap()),
            Some(&MetadataValue::String("ev_val".into()))
        );
    }
}
