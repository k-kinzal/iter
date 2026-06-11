//! `AnyQueue` enum + the `build_queue` builder.

mod kafka;
mod kinesis;
mod pubsub;
mod servicebus;
mod sqs;

use std::future::Future;
use std::time::Duration;

use iter_core::queue::azure::{ServiceBusQueue, ServiceBusQueueError};
use iter_core::queue::dlq::{DlqPolicy, DlqTarget};
use iter_core::queue::gcp::{PubSubQueue, PubSubQueueError};
use iter_core::queue::kafka::{KafkaQueue, KafkaQueueError};
use iter_core::queue::kinesis::{KinesisQueue, KinesisQueueError};
use iter_core::queue::retry::{RetryMode, RetryPolicy};
use iter_core::queue::sqs::{SqsQueue, SqsQueueError};
use iter_core::queue::{
    FileQueue, FileQueueError, InMemoryQueue, InMemoryQueueError, RedisQueue, RedisQueueError,
    ShellQueue, ShellQueueConfig, ShellQueueError,
};
use iter_core::{Priority, Queue, Signal};
use iter_language::{DlqPolicyDef, DlqTargetDef, MetadataSource, QueueDef, RetryPolicyDef};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use iter_core::queue::sqs::MetadataSource as CoreMetadataSource;

use crate::secrets::SecretsError;

/// Enum dispatch wrapper over every concrete [`iter_core::Queue`]
/// implementation shipped in the workspace.
#[derive(Debug, Clone)]
pub enum AnyQueue {
    /// In-process [`InMemoryQueue`].
    InMemory(InMemoryQueue),
    /// Directory-based [`FileQueue`] using POSIX atomic rename.
    File(FileQueue),
    /// Distributed [`RedisQueue`].
    Redis(RedisQueue),
    /// Escape-hatch [`ShellQueue`] driven by user-supplied scripts.
    Shell(ShellQueue),
    /// AWS Simple Queue Service backend.
    Sqs(SqsQueue),
    /// GCP Cloud Pub/Sub backend.
    PubSub(PubSubQueue),
    /// Apache Kafka backend.
    Kafka(KafkaQueue),
    /// AWS Kinesis Data Streams backend.
    Kinesis(KinesisQueue),
    /// Azure Service Bus backend.
    ServiceBus(ServiceBusQueue),
}

/// Aggregated error type returned by [`AnyQueue`]'s [`Queue`] impl.
#[derive(Debug, Error)]
pub enum AnyQueueError {
    /// Forwarded error from [`InMemoryQueue`].
    #[error(transparent)]
    InMemory(InMemoryQueueError),
    /// Forwarded error from [`FileQueue`].
    #[error(transparent)]
    File(FileQueueError),
    /// Forwarded error from [`RedisQueue`].
    #[error(transparent)]
    Redis(RedisQueueError),
    /// Forwarded error from [`ShellQueue`].
    #[error(transparent)]
    Shell(ShellQueueError),
    /// Forwarded error from [`SqsQueue`].
    #[error(transparent)]
    Sqs(SqsQueueError),
    /// Forwarded error from [`PubSubQueue`].
    #[error(transparent)]
    PubSub(PubSubQueueError),
    /// Forwarded error from [`KafkaQueue`].
    #[error(transparent)]
    Kafka(KafkaQueueError),
    /// Forwarded error from [`KinesisQueue`].
    #[error(transparent)]
    Kinesis(KinesisQueueError),
    /// Forwarded error from [`ServiceBusQueue`].
    #[error(transparent)]
    ServiceBus(ServiceBusQueueError),
}

/// Errors produced while constructing a concrete [`AnyQueue`] from a
/// [`QueueDef`].
///
/// This is the **build-time** counterpart to [`AnyQueueError`], which carries
/// runtime queue failures.
#[derive(Debug, Error)]
pub enum QueueBuildError {
    /// Opening the file-backed queue directory failed.
    #[error("opening file queue at {path}: {source}")]
    OpenFile {
        /// File queue path that failed to open.
        path: String,
        /// Underlying file-queue error.
        #[source]
        source: FileQueueError,
    },
    /// Connecting to the named Redis URL failed.
    #[error("connecting to redis at {url}: {source}")]
    Redis {
        /// Redis URL that failed to connect.
        url: String,
        /// Underlying Redis error.
        #[source]
        source: RedisQueueError,
    },
    /// Constructing a shell-driven queue failed.
    #[error("constructing shell queue: {0}")]
    Shell(#[from] ShellQueueError),
    /// Constructing the SQS queue client failed.
    #[error("constructing SQS queue: {0}")]
    Sqs(#[from] SqsQueueError),
    /// Constructing the Pub/Sub queue client failed.
    #[error("constructing Pub/Sub queue: {0}")]
    PubSub(#[from] PubSubQueueError),
    /// Constructing the Kafka queue client failed.
    #[error("constructing Kafka queue: {0}")]
    Kafka(#[from] KafkaQueueError),
    /// Constructing the Kinesis queue client failed.
    #[error("constructing Kinesis queue: {0}")]
    Kinesis(#[from] KinesisQueueError),
    /// Constructing the Service Bus queue client failed.
    #[error("constructing Service Bus queue: {0}")]
    ServiceBus(#[from] ServiceBusQueueError),
    /// Resolving a secret embedded in the queue declaration failed.
    #[error("resolving {label}: {source}")]
    Secret {
        /// Field label whose secret could not be resolved (e.g.
        /// `"credentials.access_key_id"`).
        label: String,
        /// Underlying [`SecretsError`].
        #[source]
        source: SecretsError,
    },
    /// The [`QueueDef`] is structurally invalid — the lowerer should have
    /// caught it before reaching here, or the user supplied an unsupported
    /// enum variant value.
    #[error("{0}")]
    Invalid(String),
}

impl QueueBuildError {
    pub(crate) fn secret(label: impl Into<String>, source: SecretsError) -> Self {
        Self::Secret {
            label: label.into(),
            source,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl Queue for AnyQueue {
    type Error = AnyQueueError;

    async fn queue(&self, signal: Signal, priority: Priority) -> Result<(), Self::Error> {
        match self {
            Self::InMemory(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::InMemory),
            Self::File(q) => q.queue(signal, priority).await.map_err(AnyQueueError::File),
            Self::Redis(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::Redis),
            Self::Shell(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::Shell),
            Self::Sqs(q) => q.queue(signal, priority).await.map_err(AnyQueueError::Sqs),
            Self::PubSub(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::PubSub),
            Self::Kafka(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::Kafka),
            Self::Kinesis(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::Kinesis),
            Self::ServiceBus(q) => q
                .queue(signal, priority)
                .await
                .map_err(AnyQueueError::ServiceBus),
        }
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
        match self {
            Self::InMemory(q) => q.dequeue(cancel).await.map_err(AnyQueueError::InMemory),
            Self::File(q) => q.dequeue(cancel).await.map_err(AnyQueueError::File),
            Self::Redis(q) => q.dequeue(cancel).await.map_err(AnyQueueError::Redis),
            Self::Shell(q) => q.dequeue(cancel).await.map_err(AnyQueueError::Shell),
            Self::Sqs(q) => q.dequeue(cancel).await.map_err(AnyQueueError::Sqs),
            Self::PubSub(q) => q.dequeue(cancel).await.map_err(AnyQueueError::PubSub),
            Self::Kafka(q) => q.dequeue(cancel).await.map_err(AnyQueueError::Kafka),
            Self::Kinesis(q) => q.dequeue(cancel).await.map_err(AnyQueueError::Kinesis),
            Self::ServiceBus(q) => q.dequeue(cancel).await.map_err(AnyQueueError::ServiceBus),
        }
    }

    // Delegate `close` so the CLI dispatch layer can close whichever
    // concrete queue was built without needing to know which variant it
    // is. The trait default is a no-op, which would otherwise silently
    // swallow close requests for InMemoryQueue — the one backend that
    // has meaningful close semantics — and defeat the "finite trigger
    // drains the runner" contract documented on Queue::close.
    async fn close(&self) -> Result<(), Self::Error> {
        match self {
            Self::InMemory(q) => q.close().await.map_err(AnyQueueError::InMemory),
            Self::File(q) => q.close().await.map_err(AnyQueueError::File),
            Self::Redis(q) => q.close().await.map_err(AnyQueueError::Redis),
            Self::Shell(q) => q.close().await.map_err(AnyQueueError::Shell),
            Self::Sqs(q) => q.close().await.map_err(AnyQueueError::Sqs),
            Self::PubSub(q) => q.close().await.map_err(AnyQueueError::PubSub),
            Self::Kafka(q) => q.close().await.map_err(AnyQueueError::Kafka),
            Self::Kinesis(q) => q.close().await.map_err(AnyQueueError::Kinesis),
            Self::ServiceBus(q) => q.close().await.map_err(AnyQueueError::ServiceBus),
        }
    }
}

/// Build an [`AnyQueue`] from a [`QueueDef`].
///
/// # Errors
///
/// Returns [`QueueBuildError`] when the underlying constructor fails (e.g. an
/// invalid queue directory or a malformed Redis URL).
pub fn build_queue(decl: &QueueDef) -> Result<AnyQueue, QueueBuildError> {
    match decl {
        QueueDef::Memory => Ok(AnyQueue::InMemory(InMemoryQueue::new())),
        QueueDef::File { path } => {
            let q = FileQueue::open(path).map_err(|source| QueueBuildError::OpenFile {
                path: path.clone(),
                source,
            })?;
            Ok(AnyQueue::File(q))
        }
        QueueDef::Redis { url, key } => {
            let url = url.clone();
            let key = key.clone();
            let q = run_async(RedisQueue::connect(&url, key)).map_err(|source| {
                QueueBuildError::Redis {
                    url: url.clone(),
                    source,
                }
            })?;
            Ok(AnyQueue::Redis(q))
        }
        QueueDef::Shell {
            enqueue,
            dequeue,
            close,
            interpreter,
            enqueue_timeout_secs,
        } => {
            let config = ShellQueueConfig {
                enqueue: enqueue.clone(),
                dequeue: dequeue.clone(),
                close: close.clone(),
                interpreter: interpreter.clone(),
                enqueue_timeout: enqueue_timeout_secs
                    .map(|s| Duration::from_secs(u64::try_from(s.max(0)).unwrap_or(0))),
            };
            // ShellQueue::new spawns a tokio task for the long-lived dequeue
            // child, so it needs a runtime. Run it on the same one-shot
            // runtime used for the Redis branch — once `new` returns, the
            // queue is self-contained and runs on whichever runtime hosts
            // the runner.
            let q = ShellQueue::new(config)?;
            Ok(AnyQueue::Shell(q))
        }
        QueueDef::Sqs(cfg) => {
            let core_cfg = sqs::build_sqs_config(cfg)?;
            let q = run_async(SqsQueue::new(core_cfg))?;
            Ok(AnyQueue::Sqs(q))
        }
        QueueDef::PubSub(cfg) => {
            let core_cfg = pubsub::build_pubsub_config(cfg)?;
            let q = PubSubQueue::new(core_cfg)?;
            Ok(AnyQueue::PubSub(q))
        }
        QueueDef::Kafka(cfg) => {
            let core_cfg = kafka::build_kafka_config(cfg)?;
            let q = KafkaQueue::new(core_cfg)?;
            Ok(AnyQueue::Kafka(q))
        }
        QueueDef::Kinesis(cfg) => {
            let core_cfg = kinesis::build_kinesis_config(cfg)?;
            let q = KinesisQueue::new(core_cfg)?;
            Ok(AnyQueue::Kinesis(q))
        }
        QueueDef::ServiceBus(cfg) => {
            let core_cfg = servicebus::build_servicebus_config(cfg)?;
            let q = ServiceBusQueue::new(core_cfg)?;
            Ok(AnyQueue::ServiceBus(q))
        }
    }
}

pub(super) fn translate_template(t: &MetadataSource) -> CoreMetadataSource {
    match t {
        MetadataSource::Literal(s) => CoreMetadataSource::Literal(s.clone()),
        MetadataSource::FromMetadata(k) => CoreMetadataSource::FromMetadata(k.clone()),
    }
}

pub(super) fn translate_retry(decl: &RetryPolicyDef) -> Result<RetryPolicy, QueueBuildError> {
    let mut p = RetryPolicy::default();
    if let Some(mode) = decl.mode.as_deref() {
        p.mode = match mode {
            "standard" => RetryMode::Standard,
            "adaptive" => RetryMode::Adaptive,
            "fixed" => RetryMode::Fixed,
            "exponential" => RetryMode::Exponential,
            other => {
                return Err(QueueBuildError::invalid(format!(
                    "unknown retry.mode `{other}`; expected one of standard | adaptive | fixed | exponential"
                )));
            }
        };
    }
    if let Some(n) = decl.max_attempts {
        p.max_attempts = u32::try_from(n.max(0)).unwrap_or(0);
    }
    if let Some(s) = decl.initial_backoff_secs {
        p.initial_backoff = secs_to_duration(s);
    }
    if let Some(s) = decl.max_backoff_secs {
        p.max_backoff = secs_to_duration(s);
    }
    if let Some(s) = decl.try_timeout_secs {
        p.try_timeout = Some(secs_to_duration(s));
    }
    p.retryable_codes.clone_from(&decl.retryable_codes);
    Ok(p)
}

pub(super) fn translate_dlq(decl: &DlqPolicyDef) -> Result<DlqPolicy, QueueBuildError> {
    let kind = decl.kind.as_deref().unwrap_or("none");
    match kind {
        "none" => Ok(DlqPolicy::None),
        "native" => Ok(DlqPolicy::Native),
        "iter_republish" => {
            let target = decl
                .target
                .as_ref()
                .ok_or_else(|| {
                    QueueBuildError::invalid(
                        "dlq.kind = \"iter_republish\" requires a target { ... } block",
                    )
                })
                .and_then(translate_dlq_target)?;
            let max_receive_count = decl
                .max_receive_count
                .map_or(5, |n| u32::try_from(n.max(0)).unwrap_or(0));
            Ok(DlqPolicy::IterRepublish {
                max_receive_count,
                target,
                include_headers: decl.include_headers.unwrap_or(true),
                reason_template: decl.reason_template.clone(),
            })
        }
        other => Err(QueueBuildError::invalid(format!(
            "unknown dlq.kind `{other}`; expected one of none | native | iter_republish"
        ))),
    }
}

fn translate_dlq_target(target: &DlqTargetDef) -> Result<DlqTarget, QueueBuildError> {
    Ok(match target {
        DlqTargetDef::Sqs { queue_url, region } => DlqTarget::Sqs {
            queue_url: queue_url.clone(),
            region: region
                .clone()
                .ok_or_else(|| QueueBuildError::invalid("dlq.target = sqs requires `region`"))?,
        },
        DlqTargetDef::Kinesis { stream_arn, region } => DlqTarget::Kinesis {
            stream_arn: stream_arn.clone(),
            region: region.clone().ok_or_else(|| {
                QueueBuildError::invalid("dlq.target = kinesis requires `region`")
            })?,
        },
        DlqTargetDef::Kafka { brokers, topic } => DlqTarget::Kafka {
            brokers: brokers.clone(),
            topic: topic.clone(),
        },
        DlqTargetDef::S3 {
            bucket,
            prefix,
            region,
        } => DlqTarget::S3 {
            bucket: bucket.clone(),
            prefix: prefix.clone().unwrap_or_default(),
            region: region
                .clone()
                .ok_or_else(|| QueueBuildError::invalid("dlq.target = s3 requires `region`"))?,
        },
        DlqTargetDef::File { path } => DlqTarget::File { path: path.clone() },
        DlqTargetDef::PubSub { project, topic } => DlqTarget::PubSub {
            project: project.clone(),
            topic: topic.clone(),
        },
        DlqTargetDef::ServiceBus { namespace, entity } => DlqTarget::ServiceBus {
            namespace: namespace.clone(),
            queue: entity.clone(),
        },
    })
}

pub(super) fn secs_to_duration(s: i64) -> Duration {
    Duration::from_secs(u64::try_from(s.max(0)).unwrap_or(0))
}

pub(super) fn ms_to_duration(ms: i64) -> Duration {
    Duration::from_millis(u64::try_from(ms.max(0)).unwrap_or(0))
}

pub(super) fn opt_u32(v: Option<i64>) -> Option<u32> {
    v.map(|n| u32::try_from(n.max(0)).unwrap_or(0))
}

pub(super) fn opt_u64(v: Option<i64>) -> Option<u64> {
    v.map(|n| u64::try_from(n.max(0)).unwrap_or(0))
}

/// Run a single async future to completion on a temporary current-thread
/// tokio runtime.
///
/// `build_queue` is sync but several queue backends expose async constructors
/// (Redis `connect`, AWS SDK `from_env`, Pub/Sub channel handshake, ...).
/// Spinning up a one-shot runtime keeps `build_queue`'s signature simple and
/// matches how the rest of the CLI bootstrap (sync `main` calling into async
/// init helpers) is structured.
fn run_async<F: Future>(fut: F) -> F::Output
where
    F::Output: Sized,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building temporary tokio runtime for queue connect");
    runtime.block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_queue_memory_returns_inmemory() {
        let q = build_queue(&QueueDef::Memory).expect("memory");
        assert!(matches!(q, AnyQueue::InMemory(_)));
    }

    #[test]
    fn build_queue_redis_invalid_url_errors() {
        let err = build_queue(&QueueDef::Redis {
            url: "not-a-real-url".into(),
            key: "iter:test".into(),
        })
        .expect_err("invalid url");
        // The exact text differs across redis crate versions; just confirm
        // the connect path was hit.
        let msg = err.to_string();
        assert!(
            msg.contains("connecting to redis") || msg.contains("redis"),
            "unexpected error: {msg}"
        );
    }
}
