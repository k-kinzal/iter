//! The `build_queue` builder — a queue **definition** becomes a runtime
//! `Arc<dyn Queue>`.
//!
//! This is "made from the full definition": every backend (including `shell`
//! and `sqs`, which carry scripts / structured config a URL cannot) is built
//! here from its [`QueueDef`]. The address/descriptor-connectable subset has a
//! second path through [`iter_core::queue::connect`].

mod sqs;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use iter_core::queue::dlq::{DlqPolicy, DlqTarget};
use iter_core::queue::retry::{RetryMode, RetryPolicy};
use iter_core::queue::sqs::{SqsQueue, SqsQueueError};
use iter_core::queue::{
    FileQueue, FileQueueError, MetadataSource as CoreMetadataSource, Queue, RedisQueue,
    RedisQueueError, ShellQueue, ShellQueueConfig, ShellQueueError,
};
use iter_core::queue::InMemoryQueue;
use iter_language::{DlqPolicyDef, DlqTargetDef, MetadataSource, QueueDef, RetryPolicyDef};
use thiserror::Error;

use crate::secrets::SecretsError;

/// Errors produced while building a concrete `Arc<dyn Queue>` from a
/// [`QueueDef`].
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

/// Build an `Arc<dyn Queue>` from a [`QueueDef`].
///
/// # Errors
///
/// Returns [`QueueBuildError`] when the underlying constructor fails (e.g. an
/// invalid queue directory or a malformed Redis URL).
pub fn build_queue(decl: &QueueDef) -> Result<Arc<dyn Queue>, QueueBuildError> {
    match decl {
        QueueDef::Memory => Ok(Arc::new(InMemoryQueue::new())),
        QueueDef::File { path } => {
            let q = FileQueue::open(path).map_err(|source| QueueBuildError::OpenFile {
                path: path.clone(),
                source,
            })?;
            Ok(Arc::new(q))
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
            Ok(Arc::new(q))
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
            // child, so it needs a runtime. Once `new` returns the queue is
            // self-contained and runs on whichever runtime hosts the runner.
            let q = ShellQueue::new(config)?;
            Ok(Arc::new(q))
        }
        QueueDef::Sqs(cfg) => {
            let core_cfg = sqs::build_sqs_config(cfg)?;
            let q = run_async(SqsQueue::new(core_cfg))?;
            Ok(Arc::new(q))
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
    })
}

pub(super) fn secs_to_duration(s: i64) -> Duration {
    Duration::from_secs(u64::try_from(s.max(0)).unwrap_or(0))
}

pub(super) fn opt_u32(v: Option<i64>) -> Option<u32> {
    v.map(|n| u32::try_from(n.max(0)).unwrap_or(0))
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

    #[tokio::test]
    async fn build_queue_memory_returns_usable_queue() {
        let q = build_queue(&QueueDef::Memory).expect("memory");
        // It is a working in-process queue: enqueue succeeds.
        q.enqueue(
            iter_core::signal::Signal::new(iter_core::signal::Metadata::new()),
            iter_core::Priority::NORMAL,
        )
        .await
        .expect("enqueue");
    }

    #[test]
    fn build_queue_redis_invalid_url_errors() {
        let err = build_queue(&QueueDef::Redis {
            url: "not-a-real-url".into(),
            key: "iter:test".into(),
        })
        .err()
        .expect("invalid url");
        // The exact text differs across redis crate versions; just confirm
        // the connect path was hit.
        let msg = err.to_string();
        assert!(
            msg.contains("connecting to redis") || msg.contains("redis"),
            "unexpected error: {msg}"
        );
    }
}
