//! [`FilesTrigger`] — emits one signal per newline-separated path.

use std::path::PathBuf;
use std::sync::Arc;

use iter_core::{Metadata, MetadataError, MetadataKey, MetadataValue, Priority, Queue, Signal};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
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

/// Persisted cursor for resuming file reads after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FilesCursor {
    offset: u64,
    version: u32,
}

const CURSOR_FILENAME: &str = "cursor.json";

/// A trigger that emits one signal per line read from stdin or a file.
///
/// Empty lines and lines beginning with `#` are skipped. The trigger exits
/// cleanly once the source is exhausted, or earlier on cancellation.
///
/// When a `state_dir` is set, the trigger persists a byte-offset cursor
/// after each emitted signal so a supervised restart can resume without
/// re-reading from the beginning. Delivery is at-least-once: a crash
/// between enqueue and cursor save may cause one line to be re-emitted.
pub struct FilesTrigger<Q: Queue + ?Sized> {
    queue: Arc<Q>,
    source: FilesSource,
    base_metadata: Metadata,
    priority: Priority,
    trigger_name: Option<String>,
    state_dir: Option<PathBuf>,
}

impl<Q: Queue + ?Sized> std::fmt::Debug for FilesTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesTrigger")
            .field("source", &self.source)
            .field("priority", &self.priority)
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + ?Sized + 'static> FilesTrigger<Q> {
    /// Build a files trigger reading from `source`.
    #[must_use]
    pub fn new(queue: Arc<Q>, source: FilesSource) -> Self {
        Self {
            queue,
            source,
            base_metadata: Metadata::new(),
            priority: Priority::NORMAL,
            trigger_name: None,
            state_dir: None,
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

    /// Set a state directory for persisting the read cursor across
    /// restarts.  When set, the trigger resumes from the last emitted
    /// offset instead of re-reading from the beginning.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = Some(dir);
        self
    }

    /// Drive the trigger until the supplied cancellation token is fired.
    ///
    /// # Errors
    ///
    /// Returns `FilesTriggerError` if reading input or queue enqueue fails.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), FilesTriggerError<iter_core::queue::QueueError>> {
        let file_key = MetadataKey::new("file")?;
        let saved_offset = self.load_cursor();

        match &self.source {
            FilesSource::Stdin => {
                let stdin = tokio::io::stdin();
                let reader = BufReader::new(stdin);
                self.drive(reader, cancel, &file_key, 0).await
            }
            FilesSource::Path(path) => {
                let mut file = tokio::fs::File::open(path).await?;
                if saved_offset > 0 {
                    file.seek(std::io::SeekFrom::Start(saved_offset)).await?;
                    tracing::info!(
                        trigger = self.trigger_name.as_deref().unwrap_or(""),
                        offset = saved_offset,
                        "resuming files trigger from persisted cursor",
                    );
                }
                let reader = BufReader::new(file);
                self.drive(reader, cancel, &file_key, saved_offset).await
            }
        }
    }

    async fn drive<R>(
        &self,
        mut reader: BufReader<R>,
        cancel: CancellationToken,
        file_key: &MetadataKey,
        start_offset: u64,
    ) -> Result<(), FilesTriggerError<iter_core::queue::QueueError>>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut offset = start_offset;
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
            offset += n as u64;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.save_cursor(offset);
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
            self.save_cursor(offset);
        }
    }

    fn load_cursor(&self) -> u64 {
        let Some(dir) = &self.state_dir else {
            return 0;
        };
        let path = dir.join(CURSOR_FILENAME);
        let Ok(data) = std::fs::read_to_string(&path) else {
            return 0;
        };
        serde_json::from_str::<FilesCursor>(&data)
            .map(|c| c.offset)
            .unwrap_or(0)
    }

    fn save_cursor(&self, offset: u64) {
        let Some(dir) = &self.state_dir else {
            return;
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!(error = %e, "failed to create files trigger state dir");
            return;
        }
        let cursor = FilesCursor { offset, version: 1 };
        match serde_json::to_string(&cursor) {
            Ok(json) => {
                let tmp = dir.join("cursor.json.tmp");
                let target = dir.join(CURSOR_FILENAME);
                if let Err(e) = std::fs::write(&tmp, &json) {
                    tracing::warn!(error = %e, "failed to write files trigger cursor");
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp, &target) {
                    tracing::warn!(error = %e, "failed to rename files trigger cursor");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize files trigger cursor");
            }
        }
    }

    async fn queue_signal(
        &self,
        signal: Signal,
        span: tracing::Span,
    ) -> Result<(), FilesTriggerError<iter_core::queue::QueueError>> {
        async move {
            let signal = iter_core::telemetry::inject_current_context_into_signal(signal);
            self.queue
                .enqueue(signal, self.priority)
                .await
                .map_err(FilesTriggerError::Queue)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use iter_core::queue::InMemoryQueue;
    use std::sync::Arc;

    #[tokio::test]
    async fn cursor_persists_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "line1\nline2\nline3\n").unwrap();
        let state_dir = dir.path().join("state");

        let queue = Arc::new(InMemoryQueue::new());
        let cancel = CancellationToken::new();

        // First run: read all lines
        let trigger = FilesTrigger::new(queue.clone(), FilesSource::Path(input.clone()))
            .with_state_dir(state_dir.clone());
        trigger.run(cancel.clone()).await.unwrap();

        // Verify cursor was saved
        let cursor_path = state_dir.join(CURSOR_FILENAME);
        assert!(cursor_path.exists(), "cursor file should exist");
        let cursor: FilesCursor =
            serde_json::from_str(&std::fs::read_to_string(&cursor_path).unwrap()).unwrap();
        assert!(cursor.offset > 0, "cursor offset should be > 0");

        // Append more data
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&input)
            .unwrap();
        writeln!(f, "line4").unwrap();
        drop(f);

        // Second run: should only read line4
        let queue2 = Arc::new(InMemoryQueue::new());
        let trigger2 =
            FilesTrigger::new(queue2.clone(), FilesSource::Path(input)).with_state_dir(state_dir);
        trigger2.run(CancellationToken::new()).await.unwrap();

        queue2.close().await.unwrap();
        let mut signals = Vec::new();
        let dq_cancel = CancellationToken::new();
        while let Ok(Some(s)) = queue2.dequeue(dq_cancel.clone()).await {
            signals.push(s);
        }
        assert_eq!(
            signals.len(),
            1,
            "second run should only emit line4, got {} signals",
            signals.len()
        );
    }
}
