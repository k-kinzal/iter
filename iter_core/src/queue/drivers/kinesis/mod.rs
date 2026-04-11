//! Amazon Kinesis Data Streams queue backend.
//!
//! Reuses the shared AWS [`credentials`](crate::queue::drivers::aws::credentials)
//! and [`http`](crate::queue::drivers::aws::http) modules for credential
//! composition and HTTP-client tuning so SQS and Kinesis share the same
//! provider chain and timeout knobs. Producer maps to `PutRecords`;
//! consumer supports both polling (`GetShardIterator` + `GetRecords`) and
//! Enhanced Fan-Out (`SubscribeToShard`). Multi-shard consumption runs
//! one async task per shard, with shard discovery via `ListShards`.
//! Checkpoint stores: `dynamodb`, `file`, `memory`. DLQ via
//! [`DlqPolicy::IterRepublish`](crate::queue::dlq::DlqPolicy::IterRepublish)
//! since Kinesis has no native DLQ.
//!
//! # Stub implementation
//!
//! Phase 3 of the queue-backend expansion lands the full DSL surface for
//! `queue kinesis { ... }` (identity, credentials, `http_client`, producer,
//! consumer with both polling and EFO modes, checkpoint stores, DLQ
//! republish targets), plus the lowerer, `AnyQueue` dispatch arm, and
//! compose-layer translation. The actual `aws-sdk-kinesis` runtime —
//! per-shard task pool, transparent shard-iterator renewal at the 5-minute
//! expiry, resharding handling, lease coordination across multiple iter
//! processes, KPL-style aggregation, iter-side DLQ republish — lands in a
//! follow-up release.
//!
//! Until that lands, [`KinesisQueue::new`] succeeds (so the runner can be
//! constructed end-to-end and the Iterfile validates) but every
//! `queue` / `dequeue` call returns
//! [`KinesisQueueError::NotYetImplemented`]. This matches the pattern used
//! for [`PubSubQueueError::NotYetImplemented`](crate::queue::drivers::pubsub::pubsub::PubSubQueueError::NotYetImplemented)
//! and [`KafkaQueueError::NotYetImplemented`](crate::queue::drivers::kafka::KafkaQueueError::NotYetImplemented).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::queue::Priority;
use crate::queue::Queue;
use crate::queue::dlq::DlqPolicy;
use crate::queue::drivers::aws::credentials::{AwsCredentials, CredentialsBuildError};
use crate::queue::drivers::aws::http::AwsHttpClientConfig;
use crate::queue::drivers::sqs::TemplatedString;
use crate::queue::retry::RetryPolicy;
use crate::signal::Signal;

/// Stream identity (ARN preferred over plain name).
#[derive(Debug, Clone)]
pub enum KinesisIdentity {
    /// Stream ARN — preferred.
    Arn(String),
    /// Plain stream name (looked up against the configured region/account).
    Name(String),
}

/// Producer-side knobs.
#[derive(Debug, Clone, Default)]
pub struct KinesisProducerConfig {
    /// Partition key source. `None` → random.
    pub partition_key_strategy: Option<TemplatedString>,
    /// Per-message explicit hash key escape hatch.
    pub explicit_hash_key: Option<String>,
    /// `none` (default) or `strict_per_key`.
    pub ordering: Option<String>,
    /// `PutRecords` batch size (1–500).
    pub batch_size: Option<u32>,
    /// `PutRecords` batch byte cap (≤ 5 MiB).
    pub batch_max_bytes: Option<u32>,
    /// Linger before flushing a partial batch.
    pub batch_linger: Option<Duration>,
    /// Iter-implemented KPL-style record aggregation.
    pub aggregation: Option<bool>,
}

/// Consumer-side knobs.
#[derive(Debug, Clone, Default)]
pub struct KinesisConsumerConfig {
    /// `polling` or `enhanced_fan_out`.
    pub consumer_mode: Option<String>,
    /// Polling iterator type or EFO starting position.
    pub iterator_type: Option<String>,
    /// Required for `AT_/AFTER_SEQUENCE_NUMBER`.
    pub starting_sequence_number: Option<String>,
    /// Required for `AT_TIMESTAMP`.
    pub starting_timestamp: Option<String>,
    /// Polling: max records per `GetRecords`.
    pub fetch_max_records: Option<u32>,
    /// Polling: poll interval.
    pub poll_interval: Option<Duration>,
    /// EFO: pre-existing consumer ARN.
    pub consumer_arn: Option<String>,
    /// EFO: registered consumer name.
    pub consumer_name: Option<String>,
    /// `ListShards` interval.
    pub shard_discovery_interval: Option<Duration>,
    /// Filter discovered shards by id list.
    pub shard_id_filter: Option<Vec<String>>,
    /// Server-side `ShardFilter` block.
    pub shard_list_filter: Option<KinesisShardListFilter>,
}

/// Server-side `ShardFilter` for `ListShards`.
#[derive(Debug, Clone, Default)]
pub struct KinesisShardListFilter {
    /// Filter type (e.g. `AT_LATEST`).
    pub kind: Option<String>,
    /// Optional shard id anchor.
    pub shard_id: Option<String>,
    /// Optional timestamp anchor.
    pub timestamp: Option<String>,
}

/// Checkpoint store configuration.
#[derive(Debug, Clone, Default)]
pub struct KinesisCheckpointConfig {
    /// `dynamodb`, `file`, or `memory`.
    pub store: Option<String>,
    /// `DynamoDB` table name (required when `store = "dynamodb"`).
    pub table_name: Option<String>,
    /// `DynamoDB` region override (defaults to stream region).
    pub region: Option<String>,
    /// `DynamoDB` endpoint override (`LocalStack`).
    pub endpoint_url: Option<String>,
    /// File path (required when `store = "file"`).
    pub path: Option<String>,
    /// Checkpoint flush interval.
    pub interval: Option<Duration>,
    /// Lease duration for multi-worker leasing.
    pub lease_duration: Option<Duration>,
}

/// Resolved Kinesis queue configuration. Compose-layer responsibility to
/// produce this from the AST (resolving `SecretExpr`, etc.) before
/// calling [`KinesisQueue::new`].
#[derive(Debug, Clone)]
pub struct KinesisQueueConfig {
    /// Stream identity.
    pub identity: KinesisIdentity,
    /// Region the stream lives in.
    pub region: Option<String>,
    /// Optional override endpoint (`LocalStack` / Kinesalite).
    pub endpoint_url: Option<String>,
    /// Resolved credential block.
    pub credentials: Option<AwsCredentials>,
    /// Resolved HTTP-client tuning.
    pub http_client: Option<AwsHttpClientConfig>,
    /// Producer-side knobs.
    pub producer: Option<KinesisProducerConfig>,
    /// Consumer-side knobs.
    pub consumer: Option<KinesisConsumerConfig>,
    /// Checkpoint store configuration.
    pub checkpoint: Option<KinesisCheckpointConfig>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicy>,
    /// Iter-implemented DLQ policy.
    pub dlq: Option<DlqPolicy>,
}

/// Errors returned by the Kinesis backend.
#[derive(Debug, Error)]
pub enum KinesisQueueError {
    /// The configuration was internally inconsistent.
    #[error("config error: {0}")]
    Config(String),
    /// Failed to build the credential provider.
    #[error("credentials: {0}")]
    Credentials(#[from] CredentialsBuildError),
    /// The Kinesis runtime path is not yet wired in the current build.
    #[error(
        "Kinesis `{operation}` is not yet implemented; the DSL surface is stable but the aws-sdk-kinesis runtime wiring lands in a follow-up release"
    )]
    NotYetImplemented {
        /// Name of the operation the caller invoked.
        operation: &'static str,
    },
    /// `queue()` was called after `close()`.
    #[error("queue is closed")]
    Closed,
}

/// Amazon Kinesis Data Streams queue.
#[derive(Debug, Clone)]
pub struct KinesisQueue {
    inner: Arc<KinesisQueueInner>,
}

#[derive(Debug)]
struct KinesisQueueInner {
    config: KinesisQueueConfig,
    closed: AtomicBool,
}

impl KinesisQueue {
    /// Construct a new Kinesis queue from a resolved config.
    ///
    /// The current build validates identity / region pairing and the
    /// checkpoint store's required fields. The AWS client itself is not
    /// yet constructed — see the module-level docs.
    ///
    /// # Errors
    ///
    /// Returns [`KinesisQueueError::Config`] when required fields are
    /// missing or inconsistent (e.g. `Name` identity without a region;
    /// checkpoint store=dynamodb without `table_name`).
    pub fn new(config: KinesisQueueConfig) -> Result<Self, KinesisQueueError> {
        match &config.identity {
            KinesisIdentity::Arn(arn) if arn.trim().is_empty() => {
                return Err(KinesisQueueError::Config(
                    "stream_arn must not be empty".into(),
                ));
            }
            KinesisIdentity::Name(name) if name.trim().is_empty() => {
                return Err(KinesisQueueError::Config(
                    "stream_name must not be empty".into(),
                ));
            }
            KinesisIdentity::Name(_) if config.region.is_none() => {
                return Err(KinesisQueueError::Config(
                    "stream_name identity requires `region`; pass stream_arn or set `region`"
                        .into(),
                ));
            }
            _ => {}
        }
        if let Some(checkpoint) = &config.checkpoint {
            match checkpoint.store.as_deref() {
                Some("dynamodb") => {
                    if checkpoint.table_name.as_deref().is_none_or(str::is_empty) {
                        return Err(KinesisQueueError::Config(
                            "checkpoint.store = dynamodb requires `table_name`".into(),
                        ));
                    }
                }
                Some("file") => {
                    if checkpoint.path.as_deref().is_none_or(str::is_empty) {
                        return Err(KinesisQueueError::Config(
                            "checkpoint.store = file requires `path`".into(),
                        ));
                    }
                }
                Some("memory") | None => {}
                Some(other) => {
                    return Err(KinesisQueueError::Config(format!(
                        "unknown checkpoint.store `{other}`; expected dynamodb | file | memory"
                    )));
                }
            }
        }
        if let Some(consumer) = &config.consumer {
            match consumer.consumer_mode.as_deref() {
                Some("polling") => {
                    if consumer.consumer_arn.is_some() || consumer.consumer_name.is_some() {
                        return Err(KinesisQueueError::Config(
                            "consumer.consumer_mode = polling cannot set consumer_arn / consumer_name (those belong to enhanced_fan_out)".into(),
                        ));
                    }
                }
                Some("enhanced_fan_out") => {
                    if consumer.consumer_arn.is_none() && consumer.consumer_name.is_none() {
                        return Err(KinesisQueueError::Config(
                            "consumer.consumer_mode = enhanced_fan_out requires consumer_arn or consumer_name".into(),
                        ));
                    }
                }
                Some(other) => {
                    return Err(KinesisQueueError::Config(format!(
                        "unknown consumer.consumer_mode `{other}`; expected polling | enhanced_fan_out"
                    )));
                }
                None => {}
            }
        }
        Ok(Self {
            inner: Arc::new(KinesisQueueInner {
                config,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Resolved stream identity.
    #[must_use]
    pub fn identity(&self) -> &KinesisIdentity {
        &self.inner.config.identity
    }
}

impl Queue for KinesisQueue {
    type Error = KinesisQueueError;

    async fn queue(&self, _signal: Signal, _priority: Priority) -> Result<(), Self::Error> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(KinesisQueueError::Closed);
        }
        Err(KinesisQueueError::NotYetImplemented { operation: "queue" })
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        Err(KinesisQueueError::NotYetImplemented {
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

    fn arn_config() -> KinesisQueueConfig {
        KinesisQueueConfig {
            identity: KinesisIdentity::Arn(
                "arn:aws:kinesis:us-east-1:123:stream/iter-signals".into(),
            ),
            region: Some("us-east-1".into()),
            endpoint_url: None,
            credentials: None,
            http_client: None,
            producer: None,
            consumer: None,
            checkpoint: None,
            retry: None,
            dlq: None,
        }
    }

    #[test]
    fn new_accepts_arn_identity() {
        let q = KinesisQueue::new(arn_config()).expect("arn config");
        assert!(matches!(q.identity(), KinesisIdentity::Arn(_)));
    }

    #[test]
    fn new_rejects_name_without_region() {
        let mut cfg = arn_config();
        cfg.identity = KinesisIdentity::Name("iter-signals".into());
        cfg.region = None;
        let err = KinesisQueue::new(cfg).expect_err("missing region");
        assert!(matches!(err, KinesisQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_dynamodb_checkpoint_without_table() {
        let mut cfg = arn_config();
        cfg.checkpoint = Some(KinesisCheckpointConfig {
            store: Some("dynamodb".into()),
            ..Default::default()
        });
        let err = KinesisQueue::new(cfg).expect_err("missing table");
        assert!(matches!(err, KinesisQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_efo_without_consumer_arn_or_name() {
        let mut cfg = arn_config();
        cfg.consumer = Some(KinesisConsumerConfig {
            consumer_mode: Some("enhanced_fan_out".into()),
            ..Default::default()
        });
        let err = KinesisQueue::new(cfg).expect_err("missing consumer arn");
        assert!(matches!(err, KinesisQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_polling_with_consumer_arn() {
        let mut cfg = arn_config();
        cfg.consumer = Some(KinesisConsumerConfig {
            consumer_mode: Some("polling".into()),
            consumer_arn: Some("arn:...".into()),
            ..Default::default()
        });
        let err = KinesisQueue::new(cfg).expect_err("polling + consumer_arn");
        assert!(matches!(err, KinesisQueueError::Config(_)));
    }

    #[tokio::test]
    async fn queue_returns_not_yet_implemented() {
        let q = KinesisQueue::new(arn_config()).expect("new");
        let signal = Signal::new(crate::signal::Metadata::new());
        let err = q
            .queue(signal, Priority::default())
            .await
            .expect_err("queue stub errors");
        assert!(matches!(
            err,
            KinesisQueueError::NotYetImplemented { operation: "queue" }
        ));
    }

    #[tokio::test]
    async fn dequeue_returns_none_on_cancel() {
        let q = KinesisQueue::new(arn_config()).expect("new");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = q.dequeue(cancel).await.expect("cancelled dequeue is Ok");
        assert!(result.is_none());
    }
}
