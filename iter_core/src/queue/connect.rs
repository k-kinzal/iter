//! [`connect`] — turn a [`QueueDescriptor`] into a usable `Arc<dyn Queue>`.
//!
//! This is the connection half of the Queue boundary: given the resolved
//! parameters another process needs, produce a live queue. The addressable
//! backends (`memory`/`file`/`redis`) connect from their address; SQS connects
//! from its structured descriptor. `ShellQueue` is deliberately *not*
//! connectable here — it needs its enqueue/dequeue scripts, so it is built
//! only from the full queue definition.

use std::sync::Arc;

use thiserror::Error;

use crate::queue::descriptor::QueueDescriptor;
use crate::queue::{FileQueue, FileQueueError, InMemoryQueue, Queue};

/// Errors connecting a [`QueueDescriptor`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConnectError {
    /// Opening the file-backed queue directory failed.
    #[error("opening file queue at {path}: {source}")]
    FileOpen {
        /// Path that failed to open.
        path: String,
        /// Underlying file-queue error.
        #[source]
        source: FileQueueError,
    },
    /// Connecting to Redis failed.
    #[cfg(feature = "driver-redis")]
    #[error("connecting to redis at {url}: {source}")]
    Redis {
        /// Redis URL that failed to connect.
        url: String,
        /// Underlying Redis error.
        #[source]
        source: crate::queue::RedisQueueError,
    },
    /// Constructing the SQS client failed.
    #[cfg(feature = "driver-sqs")]
    #[error("connecting to SQS: {0}")]
    Sqs(#[from] crate::queue::drivers::sqs::SqsQueueError),
    /// The SQS descriptor named neither a queue URL nor a
    /// `(queue_name, account_id)` pair.
    #[error("SQS descriptor has neither queue_url nor (queue_name, account_id)")]
    SqsIdentityMissing,
    /// The descriptor names a backend that is not compiled into this build.
    #[error("queue backend `{0}` is not compiled into this build")]
    BackendUnavailable(&'static str),
}

/// Connect to the queue a [`QueueDescriptor`] names, returning it as an
/// `Arc<dyn Queue>`.
///
/// # Errors
///
/// Returns [`ConnectError`] when the underlying connection fails or the
/// descriptor is structurally incomplete (e.g. an SQS descriptor with no
/// identity), or when the named backend was not compiled in.
pub async fn connect(descriptor: &QueueDescriptor) -> Result<Arc<dyn Queue>, ConnectError> {
    match descriptor {
        QueueDescriptor::Memory => Ok(Arc::new(InMemoryQueue::new())),
        QueueDescriptor::File { path } => {
            let q = FileQueue::open(path).map_err(|source| ConnectError::FileOpen {
                path: path.clone(),
                source,
            })?;
            Ok(Arc::new(q))
        }
        QueueDescriptor::Redis { url, key } => connect_redis(url, key).await,
        QueueDescriptor::Sqs(descriptor) => connect_sqs(descriptor).await,
    }
}

#[cfg(feature = "driver-redis")]
async fn connect_redis(url: &str, key: &str) -> Result<Arc<dyn Queue>, ConnectError> {
    let q = crate::queue::RedisQueue::connect(url, key.to_string())
        .await
        .map_err(|source| ConnectError::Redis {
            url: url.to_string(),
            source,
        })?;
    Ok(Arc::new(q))
}

#[cfg(not(feature = "driver-redis"))]
async fn connect_redis(_url: &str, _key: &str) -> Result<Arc<dyn Queue>, ConnectError> {
    Err(ConnectError::BackendUnavailable("redis"))
}

#[cfg(feature = "driver-sqs")]
async fn connect_sqs(
    descriptor: &crate::queue::descriptor::SqsDescriptor,
) -> Result<Arc<dyn Queue>, ConnectError> {
    use crate::queue::drivers::aws::credentials::AwsCredentials;
    use crate::queue::drivers::sqs::{SqsIdentity, SqsProducerConfig, SqsQueue, SqsQueueConfig};

    let identity = if let Some(url) = &descriptor.queue_url {
        SqsIdentity::Url(url.clone())
    } else if let (Some(name), Some(account_id)) = (&descriptor.queue_name, &descriptor.account_id)
    {
        SqsIdentity::NameWithAccount {
            name: name.clone(),
            account_id: account_id.clone(),
        }
    } else {
        return Err(ConnectError::SqsIdentityMissing);
    };

    let credentials = descriptor
        .credentials
        .as_ref()
        .map(|c| AwsCredentials::Static {
            access_key_id: c.access_key_id.clone(),
            secret_access_key: c.secret_access_key.clone(),
            session_token: c.session_token.clone(),
        });

    let producer = descriptor
        .message_group_id
        .as_ref()
        .map(|m| SqsProducerConfig {
            message_group_id: Some(m.clone()),
            ..SqsProducerConfig::default()
        });

    let config = SqsQueueConfig {
        identity,
        region: descriptor.region.clone(),
        endpoint_url: descriptor.endpoint_url.clone(),
        fifo: descriptor.fifo,
        use_fips: None,
        use_dual_stack: None,
        sts_regional_endpoints: None,
        app_name: None,
        credentials,
        http_client: None,
        producer,
        consumer: None,
        retry: None,
        dlq: None,
    };
    let q = SqsQueue::new(config).await?;
    Ok(Arc::new(q))
}

#[cfg(not(feature = "driver-sqs"))]
async fn connect_sqs(
    _descriptor: &crate::queue::descriptor::SqsDescriptor,
) -> Result<Arc<dyn Queue>, ConnectError> {
    Err(ConnectError::BackendUnavailable("sqs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_memory_returns_a_usable_queue() {
        let q = connect(&QueueDescriptor::Memory).await.expect("memory");
        // A freshly-connected memory queue accepts an enqueue.
        q.enqueue(
            crate::signal::Signal::new(crate::signal::Metadata::new()),
            crate::queue::Priority::NORMAL,
        )
        .await
        .expect("enqueue");
    }

    #[tokio::test]
    async fn connect_file_opens_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = QueueDescriptor::File {
            path: dir.path().display().to_string(),
        };
        let _q: Arc<dyn Queue> = connect(&descriptor).await.expect("file");
    }
}
