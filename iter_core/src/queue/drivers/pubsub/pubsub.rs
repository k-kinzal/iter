//! GCP Cloud Pub/Sub queue backend.
//!
//! # Stub implementation
//!
//! Phase 2 of the queue-backend expansion ships the full DSL surface and
//! semantic lowerer for `queue pubsub { ... }`, plus the `AnyQueue` dispatch
//! arm and config translation in the compose layer. The actual
//! `google-cloud-pubsub` runtime — gRPC channel construction, publisher
//! batcher, streaming-pull subscriber, ack management, ordering keys,
//! retry/backoff — lands in a follow-up release.
//!
//! Until that lands, [`PubSubQueue::new`] succeeds (so the runner can be
//! constructed end-to-end and the Iterfile validates) but every
//! `queue` / `dequeue` call returns
//! [`PubSubQueueError::NotYetImplemented`]. This matches the established
//! "ship the DSL surface, fill in the wiring iteratively" pattern documented
//! on [`PubSubCredentialsError::NotYetImplemented`](super::credentials::PubSubCredentialsError).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::queue::Priority;
use crate::queue::Queue;
use crate::queue::dlq::DlqPolicy;
use crate::queue::drivers::pubsub::credentials::{PubSubCredentials, PubSubCredentialsError};
use crate::queue::retry::RetryPolicy;
use crate::signal::Signal;

/// Resolved Pub/Sub publisher knobs.
#[derive(Debug, Clone, Default)]
pub struct PubSubPublisherConfig {
    /// Batch flush delay (millisecond resolution honoured).
    pub delay_threshold: Option<std::time::Duration>,
    /// Flush after this many buffered messages.
    pub count_threshold: Option<u32>,
    /// Flush after this many buffered bytes.
    pub byte_threshold: Option<u32>,
    /// Backpressure cap on outstanding messages.
    pub max_outstanding_messages: Option<u32>,
    /// Backpressure cap on outstanding bytes.
    pub max_outstanding_bytes: Option<u64>,
    /// `block` (default) or `error` when the cap is hit.
    pub limit_exceeded_behavior: Option<String>,
    /// Worker thread count.
    pub workers: Option<u32>,
    /// Per-publish RPC timeout.
    pub request_timeout: Option<std::time::Duration>,
    /// Retry policy for publish RPCs.
    pub retry: Option<RetryPolicy>,
    /// Enable gRPC compression.
    pub enable_compression: Option<bool>,
    /// Minimum payload size to compress.
    pub compression_bytes_threshold: Option<u32>,
    /// Static attribute overlay.
    pub attributes: Option<Vec<(String, String)>>,
    /// Ordering key source. `None` disables ordered publishing.
    pub ordering_key_metadata: Option<String>,
}

/// Resolved Pub/Sub subscriber knobs.
#[derive(Debug, Clone, Default)]
pub struct PubSubSubscriberConfig {
    /// `streaming` (default) or `sync` pull mode.
    pub pull_mode: Option<String>,
    /// Streaming-only: extended ack deadline in seconds (10–600).
    pub stream_ack_deadline_seconds: Option<u32>,
    /// Streaming-only: outstanding-message backpressure cap.
    pub max_outstanding_messages: Option<u32>,
    /// Streaming-only: outstanding-byte backpressure cap.
    pub max_outstanding_bytes: Option<u64>,
    /// Streaming-only: minimum lease-extension interval.
    pub min_duration_per_lease_extension: Option<std::time::Duration>,
    /// Streaming-only: maximum lease-extension interval.
    pub max_duration_per_lease_extension: Option<std::time::Duration>,
    /// Streaming-only: keepalive ping interval.
    pub ping_interval: Option<std::time::Duration>,
    /// Sync-only: max messages per pull (≤ 1000).
    pub max_messages: Option<u32>,
    /// Sync-only: return immediately on empty pull.
    pub return_immediately: Option<bool>,
    /// Retry policy for receive RPCs.
    pub retry: Option<RetryPolicy>,
}

/// Idempotent startup seek operation.
#[derive(Debug, Clone)]
pub enum PubSubInitialSeek {
    /// Seek to a wall-clock instant (RFC3339 in the AST).
    Timestamp(String),
    /// Seek to a named snapshot.
    Snapshot(String),
}

/// gRPC channel keepalive parameters.
#[derive(Debug, Clone, Default)]
pub struct PubSubKeepalive {
    /// Idle time before a keepalive ping.
    pub time: Option<std::time::Duration>,
    /// Ack-deadline for the keepalive ping.
    pub timeout: Option<std::time::Duration>,
    /// Allow keepalive pings on idle channels.
    pub permit_without_stream: Option<bool>,
}

/// Resolved Pub/Sub queue configuration. Compose-layer responsibility to
/// produce this from the AST (resolving `SecretExpr`, etc.) before
/// calling [`PubSubQueue::new`].
#[derive(Debug, Clone)]
pub struct PubSubQueueConfig {
    /// GCP project hosting the topic and subscription.
    pub project: String,
    /// Topic id used by [`Queue::queue`].
    pub topic: String,
    /// Subscription id used by [`Queue::dequeue`].
    pub subscription: String,
    /// Optional regional endpoint or emulator host.
    pub endpoint: Option<String>,
    /// Optional User-Agent override.
    pub user_agent: Option<String>,
    /// Connection timeout.
    pub connect_timeout: Option<std::time::Duration>,
    /// Per-request timeout.
    pub request_timeout: Option<std::time::Duration>,
    /// gRPC channel keepalive knobs.
    pub keepalive: Option<PubSubKeepalive>,
    /// Quota project to bill API calls against.
    pub quota_project: Option<String>,
    /// OAuth scopes; defaults to the `pubsub` scope when absent.
    pub scopes: Option<Vec<String>>,
    /// Resolved credential block.
    pub credentials: Option<PubSubCredentials>,
    /// Producer-side knobs.
    pub publisher: Option<PubSubPublisherConfig>,
    /// Consumer-side knobs.
    pub subscriber: Option<PubSubSubscriberConfig>,
    /// Optional idempotent startup seek operation.
    pub initial_seek: Option<PubSubInitialSeek>,
    /// Dead-letter handling.
    pub dlq: Option<DlqPolicy>,
}

/// Errors returned by the Pub/Sub backend.
#[derive(Debug, Error)]
pub enum PubSubQueueError {
    /// The configuration was internally inconsistent.
    #[error("config error: {0}")]
    Config(String),
    /// Failed to validate the credential block.
    #[error("credentials: {0}")]
    Credentials(#[from] PubSubCredentialsError),
    /// The Pub/Sub runtime path is not yet wired in the current build.
    #[error(
        "Pub/Sub `{operation}` is not yet implemented; the DSL surface is stable but the runtime wiring lands in a follow-up release"
    )]
    NotYetImplemented {
        /// Name of the operation the caller invoked.
        operation: &'static str,
    },
    /// `queue()` was called after `close()`.
    #[error("queue is closed")]
    Closed,
}

/// GCP Cloud Pub/Sub queue.
#[derive(Debug, Clone)]
pub struct PubSubQueue {
    inner: Arc<PubSubQueueInner>,
}

#[derive(Debug)]
struct PubSubQueueInner {
    config: PubSubQueueConfig,
    closed: AtomicBool,
}

impl PubSubQueue {
    /// Construct a new Pub/Sub queue from a resolved config.
    ///
    /// The current build validates the credential surface eagerly so users
    /// get immediate feedback when they target an as-yet-unsupported
    /// credential variant. Topic / subscription identity is also
    /// validated for non-emptiness.
    ///
    /// # Errors
    ///
    /// Returns [`PubSubQueueError::Config`] when required fields are
    /// empty, and [`PubSubQueueError::Credentials`] when the chosen
    /// credential variant cannot yet be resolved.
    pub fn new(config: PubSubQueueConfig) -> Result<Self, PubSubQueueError> {
        if config.project.trim().is_empty() {
            return Err(PubSubQueueError::Config("project must not be empty".into()));
        }
        if config.topic.trim().is_empty() {
            return Err(PubSubQueueError::Config("topic must not be empty".into()));
        }
        if config.subscription.trim().is_empty() {
            return Err(PubSubQueueError::Config(
                "subscription must not be empty".into(),
            ));
        }
        if let Some(creds) = &config.credentials {
            creds.validate()?;
        }
        Ok(Self {
            inner: Arc::new(PubSubQueueInner {
                config,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Resolved fully-qualified topic name (`projects/{p}/topics/{t}`).
    #[must_use]
    pub fn topic_name(&self) -> String {
        format!(
            "projects/{}/topics/{}",
            self.inner.config.project, self.inner.config.topic
        )
    }

    /// Resolved fully-qualified subscription name.
    #[must_use]
    pub fn subscription_name(&self) -> String {
        format!(
            "projects/{}/subscriptions/{}",
            self.inner.config.project, self.inner.config.subscription
        )
    }
}

impl Queue for PubSubQueue {
    type Error = PubSubQueueError;

    async fn queue(&self, _signal: Signal, _priority: Priority) -> Result<(), Self::Error> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(PubSubQueueError::Closed);
        }
        Err(PubSubQueueError::NotYetImplemented { operation: "queue" })
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        Err(PubSubQueueError::NotYetImplemented {
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

    fn minimal_config() -> PubSubQueueConfig {
        PubSubQueueConfig {
            project: "p".into(),
            topic: "t".into(),
            subscription: "s".into(),
            endpoint: None,
            user_agent: None,
            connect_timeout: None,
            request_timeout: None,
            keepalive: None,
            quota_project: None,
            scopes: None,
            credentials: Some(PubSubCredentials::Adc),
            publisher: None,
            subscriber: None,
            initial_seek: None,
            dlq: None,
        }
    }

    #[tokio::test]
    async fn new_rejects_empty_project() {
        let mut cfg = minimal_config();
        cfg.project = String::new();
        let err = PubSubQueue::new(cfg).expect_err("empty project");
        assert!(matches!(err, PubSubQueueError::Config(_)));
    }

    #[tokio::test]
    async fn queue_returns_not_yet_implemented() {
        let q = PubSubQueue::new(minimal_config()).expect("new");
        let signal = Signal::new(crate::signal::Metadata::new());
        let err = q
            .queue(signal, Priority::default())
            .await
            .expect_err("queue stub errors");
        assert!(matches!(
            err,
            PubSubQueueError::NotYetImplemented { operation: "queue" }
        ));
    }

    #[tokio::test]
    async fn dequeue_returns_none_on_cancel() {
        let q = PubSubQueue::new(minimal_config()).expect("new");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = q.dequeue(cancel).await.expect("cancelled dequeue is Ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn dequeue_after_close_returns_none() {
        let q = PubSubQueue::new(minimal_config()).expect("new");
        q.close().await.expect("close");
        let cancel = CancellationToken::new();
        let result = q.dequeue(cancel).await.expect("closed dequeue is Ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn topic_and_subscription_names_are_fully_qualified() {
        let q = PubSubQueue::new(minimal_config()).expect("new");
        assert_eq!(q.topic_name(), "projects/p/topics/t");
        assert_eq!(q.subscription_name(), "projects/p/subscriptions/s");
    }
}
