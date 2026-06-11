//! Queue connection from a URL.

use async_trait::async_trait;
use iter_core::queue::{FileQueue, InMemoryQueue, Priority, Queue, QueueError};
use iter_core::signal::Signal;
use percent_encoding::percent_decode_str;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Errors produced while connecting to a queue by URL.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QueueLoadError {
    /// The URL scheme is not recognized.
    #[error("unsupported queue URL scheme: {0}")]
    UnsupportedScheme(String),
    /// `file://` URL with an empty path.
    #[error("file:// URL requires a non-empty path")]
    FileUrlMissingPath,
    /// Opening the file queue directory failed.
    #[error("opening file queue at {path}: {source}")]
    FileOpen {
        /// Path that failed.
        path: String,
        /// Underlying error.
        #[source]
        source: iter_core::queue::FileQueueError,
    },
    /// Connecting to Redis failed.
    #[cfg(feature = "driver-redis")]
    #[error("connecting to Redis at {url}: {source}")]
    Redis {
        /// Redis URL.
        url: String,
        /// Underlying error.
        #[source]
        source: iter_core::queue::RedisQueueError,
    },
}

/// Opaque queue handle returned by [`QueueLoader::from_url`].
///
/// Wraps whichever concrete queue type the URL resolved to.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum QueueHandle {
    /// In-process queue.
    Memory(InMemoryQueue),
    /// File-based queue.
    File(FileQueue),
    /// Redis queue.
    #[cfg(feature = "driver-redis")]
    Redis(iter_core::queue::RedisQueue),
}

#[async_trait]
impl Queue for QueueHandle {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        match self {
            Self::Memory(q) => q.enqueue(signal, priority).await,
            Self::File(q) => q.enqueue(signal, priority).await,
            #[cfg(feature = "driver-redis")]
            Self::Redis(q) => q.enqueue(signal, priority).await,
        }
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        match self {
            Self::Memory(q) => q.dequeue(cancel).await,
            Self::File(q) => q.dequeue(cancel).await,
            #[cfg(feature = "driver-redis")]
            Self::Redis(q) => q.dequeue(cancel).await,
        }
    }

    async fn close(&self) -> Result<(), QueueError> {
        match self {
            Self::Memory(q) => q.close().await,
            Self::File(q) => q.close().await,
            #[cfg(feature = "driver-redis")]
            Self::Redis(q) => q.close().await,
        }
    }
}

/// Connects to a queue by URL.
pub struct QueueLoader;

impl QueueLoader {
    /// Build a queue handle from a connection URL.
    ///
    /// Supported schemes:
    /// - `memory://` — in-process queue.
    /// - `file://<path>` — directory-based queue.
    /// - `redis://<host>:<port>[?key=<key>]` — Redis sorted-set queue.
    /// - `rediss://...` — Redis over TLS.
    ///
    /// # Errors
    ///
    /// Returns [`QueueLoadError`] if the scheme is unknown or the
    /// underlying constructor fails.
    pub async fn from_url(url: &str) -> Result<QueueHandle, QueueLoadError> {
        if url == "memory://" || url == "memory:" {
            return Ok(QueueHandle::Memory(InMemoryQueue::new()));
        }

        if let Some(rest) = url.strip_prefix("file://") {
            if rest.is_empty() {
                return Err(QueueLoadError::FileUrlMissingPath);
            }
            let q = FileQueue::open(rest).map_err(|source| QueueLoadError::FileOpen {
                path: rest.to_string(),
                source,
            })?;
            return Ok(QueueHandle::File(q));
        }

        #[cfg(feature = "driver-redis")]
        if url.starts_with("redis://") || url.starts_with("rediss://") {
            let (url_part, key) = match url.split_once('?') {
                Some((u, query)) => {
                    let mut key = "iter".to_string();
                    for pair in query.split('&') {
                        if let Some((k, v)) = pair.split_once('=') {
                            if k == "key" {
                                key = percent_decode_str(v).decode_utf8_lossy().into_owned();
                            }
                        }
                    }
                    (u.to_string(), key)
                }
                None => (url.to_string(), "iter".to_string()),
            };
            let q = iter_core::queue::RedisQueue::connect(&url_part, key)
                .await
                .map_err(|source| QueueLoadError::Redis {
                    url: url_part,
                    source,
                })?;
            return Ok(QueueHandle::Redis(q));
        }

        Err(QueueLoadError::UnsupportedScheme(url.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_url() {
        let q = QueueLoader::from_url("memory://").await.expect("memory");
        assert!(matches!(q, QueueHandle::Memory(_)));
    }

    #[tokio::test]
    async fn file_url() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("file://{}", dir.path().display());
        let q = QueueLoader::from_url(&url).await.expect("file");
        assert!(matches!(q, QueueHandle::File(_)));
    }

    #[tokio::test]
    async fn file_url_empty_path() {
        let err = QueueLoader::from_url("file://").await.expect_err("empty");
        assert!(matches!(err, QueueLoadError::FileUrlMissingPath));
    }

    #[tokio::test]
    async fn unsupported_scheme() {
        let err = QueueLoader::from_url("ftp://host").await.expect_err("ftp");
        assert!(matches!(err, QueueLoadError::UnsupportedScheme(_)));
    }
}
