//! Apache Kafka-backed queue implementation.
//!
//! Built on `rdkafka` (which links `librdkafka`). The producer maps to a
//! `FutureProducer`, the consumer to a `StreamConsumer` with manual offset
//! commit. Receive-count tracking for
//! [`DlqPolicy::IterRepublish`](super::DlqPolicy) piggybacks on an
//! `x-iter-receive-count` header iter increments on each delivery. The
//! `AWS_MSK_IAM` SASL mechanism wires an OAUTHBEARER refresh callback that
//! signs token requests with `SigV4`.
//!
//! # Stub implementation
//!
//! Phase 2 of the queue-backend expansion lands the full DSL surface for
//! `queue kafka { ... }` — every security knob, producer / consumer
//! field, exactly-once shorthand, and the untyped `extra_config`
//! escape hatch — together with the lowerer, `AnyQueue` dispatch arm, and
//! compose-layer translation. The actual `rdkafka` runtime
//! (`ClientConfig` materialisation, producer / consumer task spawn,
//! manual offset commit, MSK IAM token refresh, iter-side DLQ
//! republish) lands in a follow-up release.
//!
//! Until that lands, [`KafkaQueue::new`] succeeds (so the runner can be
//! constructed end-to-end and the Iterfile validates) but every
//! `queue` / `dequeue` call returns
//! [`KafkaQueueError::NotYetImplemented`]. This matches the pattern
//! used for [`PubSubQueueError::NotYetImplemented`](super::gcp::pubsub::PubSubQueueError::NotYetImplemented).

pub mod config;
pub mod consumer;
pub mod error;
pub mod producer;
pub mod security;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio_util::sync::CancellationToken;

use crate::queue::{Priority, Queue};
use crate::signal::Signal;

pub use config::KafkaQueueConfig;
pub use consumer::KafkaConsumerConfig;
pub use error::KafkaQueueError;
pub use producer::KafkaProducerConfig;
pub use security::KafkaSecurityConfig;

/// Apache Kafka queue.
#[derive(Debug, Clone)]
pub struct KafkaQueue {
    inner: Arc<KafkaQueueInner>,
}

#[derive(Debug)]
struct KafkaQueueInner {
    config: KafkaQueueConfig,
    closed: AtomicBool,
}

impl KafkaQueue {
    /// Construct a new Kafka queue from a resolved config.
    ///
    /// The current build validates that `bootstrap_servers` is non-empty
    /// and that producer / consumer required fields are present when
    /// the corresponding block is present. The `librdkafka` client itself
    /// is not yet constructed — see the module-level docs.
    ///
    /// # Errors
    ///
    /// Returns [`KafkaQueueError::Config`] when required fields are
    /// missing.
    pub fn new(config: KafkaQueueConfig) -> Result<Self, KafkaQueueError> {
        if config.bootstrap_servers.trim().is_empty() {
            return Err(KafkaQueueError::Config(
                "bootstrap_servers must not be empty".into(),
            ));
        }
        if let Some(consumer) = &config.consumer {
            if consumer.group_id.as_deref().is_none_or(str::is_empty) {
                return Err(KafkaQueueError::Config(
                    "consumer.group_id is required when a consumer block is present".into(),
                ));
            }
            if consumer.topics.as_ref().is_none_or(Vec::is_empty) {
                return Err(KafkaQueueError::Config(
                    "consumer.topics must list at least one topic when a consumer block is present"
                        .into(),
                ));
            }
        }
        if let Some(producer) = &config.producer {
            if producer.topic.as_deref().is_none_or(str::is_empty) {
                return Err(KafkaQueueError::Config(
                    "producer.topic is required when a producer block is present".into(),
                ));
            }
        }
        Ok(Self {
            inner: Arc::new(KafkaQueueInner {
                config,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Resolved bootstrap brokers string.
    #[must_use]
    pub fn bootstrap_servers(&self) -> &str {
        &self.inner.config.bootstrap_servers
    }
}

impl Queue for KafkaQueue {
    type Error = KafkaQueueError;

    async fn queue(&self, _signal: Signal, _priority: Priority) -> Result<(), Self::Error> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(KafkaQueueError::Closed);
        }
        Err(KafkaQueueError::NotYetImplemented { operation: "queue" })
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        Err(KafkaQueueError::NotYetImplemented {
            operation: "dequeue",
        })
    }

    async fn close(&self) -> Result<(), Self::Error> {
        self.inner.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> KafkaQueueConfig {
        KafkaQueueConfig {
            bootstrap_servers: "localhost:9092".into(),
            client_id: None,
            client_rack: None,
            broker_address_family: None,
            broker_address_ttl_secs: None,
            metadata_max_age_secs: None,
            topic_metadata_refresh_interval_secs: None,
            topic_metadata_refresh_fast_interval_ms: None,
            socket_timeout_secs: None,
            socket_keepalive_enable: None,
            socket_nagle_disable: None,
            socket_max_fails: None,
            reconnect_backoff_ms: None,
            reconnect_backoff_max_ms: None,
            api_version_request: None,
            api_version_request_timeout_ms: None,
            security: None,
            producer: None,
            consumer: None,
            exactly_once: false,
            extra_config: None,
            dlq: None,
        }
    }

    #[test]
    fn new_rejects_empty_bootstrap_servers() {
        let mut cfg = minimal_config();
        cfg.bootstrap_servers = String::new();
        let err = KafkaQueue::new(cfg).expect_err("empty bootstrap");
        assert!(matches!(err, KafkaQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_consumer_without_group_id() {
        let mut cfg = minimal_config();
        cfg.consumer = Some(KafkaConsumerConfig {
            topics: Some(vec!["t".into()]),
            ..Default::default()
        });
        let err = KafkaQueue::new(cfg).expect_err("missing group_id");
        assert!(matches!(err, KafkaQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_producer_without_topic() {
        let mut cfg = minimal_config();
        cfg.producer = Some(KafkaProducerConfig::default());
        let err = KafkaQueue::new(cfg).expect_err("missing topic");
        assert!(matches!(err, KafkaQueueError::Config(_)));
    }

    #[tokio::test]
    async fn queue_returns_not_yet_implemented() {
        let q = KafkaQueue::new(minimal_config()).expect("new");
        let signal = Signal::new(crate::signal::Metadata::new());
        let err = q
            .queue(signal, Priority::default())
            .await
            .expect_err("queue stub errors");
        assert!(matches!(
            err,
            KafkaQueueError::NotYetImplemented { operation: "queue" }
        ));
    }

    #[tokio::test]
    async fn dequeue_returns_none_on_cancel() {
        let q = KafkaQueue::new(minimal_config()).expect("new");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = q.dequeue(cancel).await.expect("cancelled dequeue is Ok");
        assert!(result.is_none());
    }
}
