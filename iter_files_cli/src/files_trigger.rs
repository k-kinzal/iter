//! [`FilesTrigger`] — emits one signal per newline-separated path.

use std::path::PathBuf;
use std::sync::Arc;

use iter_core::{Metadata, MetadataError, MetadataKey, MetadataValue, Priority, Queue, Signal};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// Where the [`FilesTrigger`] should read its path list from.
#[derive(Debug, Clone)]
pub enum FilesSource {
    /// Read newline-separated paths from process stdin.
    Stdin,
    /// Read newline-separated paths from a file.
    Path(PathBuf),
}

/// Errors produced by [`FilesTrigger`].
#[derive(Debug, Error)]
pub enum FilesTriggerError<E: std::error::Error + Send + Sync + 'static> {
    /// Forwarded error from the queue backing the trigger.
    #[error("queue error: {0}")]
    Queue(#[source] E),

    /// I/O error reading the source.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Construction of an internal metadata key failed.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),
}

/// A trigger that emits one signal per line read from stdin or a file.
///
/// Empty lines and lines beginning with `#` are skipped. The trigger exits
/// cleanly once the source is exhausted, or earlier on cancellation.
pub struct FilesTrigger<Q: Queue> {
    queue: Arc<Q>,
    source: FilesSource,
    base_metadata: Metadata,
    priority: Priority,
    trigger_name: Option<String>,
}

impl<Q: Queue> std::fmt::Debug for FilesTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesTrigger")
            .field("source", &self.source)
            .field("priority", &self.priority)
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + 'static> FilesTrigger<Q> {
    /// Build a files trigger reading from `source`.
    #[must_use]
    pub fn new(queue: Arc<Q>, source: FilesSource) -> Self {
        Self {
            queue,
            source,
            base_metadata: Metadata::new(),
            priority: Priority::NORMAL,
            trigger_name: None,
        }
    }

    /// Replace the base metadata copied into every emitted signal.
    #[must_use]
    pub fn with_base_metadata(mut self, m: Metadata) -> Self {
        self.base_metadata = m;
        self
    }

    /// Override the priority used when enqueuing emitted signals.
    #[must_use]
    pub fn with_priority(mut self, p: Priority) -> Self {
        self.priority = p;
        self
    }

    /// Attach the configured trigger name to emitted spans.
    #[must_use]
    pub fn with_trigger_name(mut self, name: impl Into<String>) -> Self {
        self.trigger_name = Some(name.into());
        self
    }

    /// Drive the trigger until the supplied cancellation token is fired.
    ///
    /// # Errors
    ///
    /// Returns `FilesTriggerError` if reading input or queue enqueue fails.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), FilesTriggerError<Q::Error>> {
        let file_key = MetadataKey::new("file")?;

        // We can't use a single trait object cleanly because the two readers
        // have different concrete types; instead, drive each separately.
        match &self.source {
            FilesSource::Stdin => {
                let stdin = tokio::io::stdin();
                let reader = BufReader::new(stdin);
                self.drive(reader, cancel, &file_key).await
            }
            FilesSource::Path(path) => {
                let file = tokio::fs::File::open(path).await?;
                let reader = BufReader::new(file);
                self.drive(reader, cancel, &file_key).await
            }
        }
    }

    async fn drive<R>(
        &self,
        mut reader: BufReader<R>,
        cancel: CancellationToken,
        file_key: &MetadataKey,
    ) -> Result<(), FilesTriggerError<Q::Error>>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut line = String::new();
        loop {
            line.clear();
            let read_fut = reader.read_line(&mut line);
            tokio::pin!(read_fut);
            let n = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                res = &mut read_fut => res?,
            };
            if n == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut metadata = self.base_metadata.clone();
            metadata.insert(file_key.clone(), MetadataValue::String(trimmed.to_owned()));
            let signal = Signal::new(metadata);
            let signal_id = signal.id();
            self.queue_signal(
                signal,
                tracing::info_span!(
                    "iter.trigger.files.emit",
                    iter.trigger.kind = "files",
                    iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                    iter.signal.id = %signal_id,
                    iter.files.path = %trimmed,
                ),
            )
            .await?;
        }
    }

    async fn queue_signal(
        &self,
        signal: Signal,
        span: tracing::Span,
    ) -> Result<(), FilesTriggerError<Q::Error>> {
        async move {
            let mut signal = signal;
            iter_core::telemetry::inject_current_context_into_signal(&mut signal);
            self.queue
                .queue(signal, self.priority)
                .await
                .map_err(FilesTriggerError::Queue)
        }
        .instrument(span)
        .await
    }
}
