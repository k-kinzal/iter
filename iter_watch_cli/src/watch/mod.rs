//! [`WatchTrigger`] — emits signals from filesystem change events.

mod config;
mod debounce;
mod filter;
mod kind;

pub use config::{WatchBackend, WatchConfig};

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use iter_core::{Metadata, MetadataError, MetadataKey, MetadataValue, Priority, Queue, Signal};
use notify::{Config as NotifyConfig, PollWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use debounce::PathDebouncer;
use filter::path_matches;
use kind::{ChangeKind, ChangeRecord};

/// Errors produced by [`WatchTrigger`].
#[derive(Debug, Error)]
pub enum WatchTriggerError<E: std::error::Error + Send + Sync + 'static> {
    /// Forwarded error from the queue backing the trigger.
    #[error("queue error: {0}")]
    Queue(#[source] E),

    /// The underlying `notify` watcher failed.
    #[error("watcher error: {0}")]
    Notify(#[from] notify::Error),

    /// Construction of an internal metadata key failed.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),

    /// Failed to serialise the batch file list to JSON.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A glob pattern stored in the config rejected compilation. In practice
    /// the patterns are validated up-front by [`WatchConfig::new`]; this
    /// variant exists as a defensive fallthrough.
    #[error("invalid glob pattern: {0}")]
    Pattern(#[from] globset::Error),
}

/// A filesystem watcher trigger.
///
/// Backed by the `notify` crate. The watcher runs on its dedicated OS thread
/// and forwards events through a tokio channel so the trigger task can stay
/// fully async.
pub struct WatchTrigger<Q: Queue> {
    queue: Arc<Q>,
    config: WatchConfig,
    base_metadata: Metadata,
    priority: Priority,
    backend: WatchBackend,
    trigger_name: Option<String>,
}

impl<Q: Queue> std::fmt::Debug for WatchTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchTrigger")
            .field("config", &self.config)
            .field("priority", &self.priority)
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + 'static> WatchTrigger<Q> {
    /// Create a watch trigger publishing to `queue` with the given `config`.
    #[must_use]
    pub fn new(queue: Arc<Q>, config: WatchConfig) -> Self {
        Self {
            queue,
            config,
            base_metadata: Metadata::new(),
            priority: Priority::NORMAL,
            backend: WatchBackend::default(),
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

    /// Select the watcher backend. Defaults to
    /// [`WatchBackend::Recommended`].
    #[allow(dead_code)]
    #[must_use]
    pub fn with_backend(mut self, backend: WatchBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Drive the trigger until the supplied cancellation token is fired.
    ///
    /// # Errors
    ///
    /// Returns `WatchTriggerError` if filesystem watching or queue enqueue fails.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), WatchTriggerError<Q::Error>> {
        let path_key = MetadataKey::new("path")?;
        let kind_key = MetadataKey::new("kind")?;
        let timestamp_key = MetadataKey::new("timestamp")?;
        let files_key = MetadataKey::new("files")?;

        let (tx, mut rx) = mpsc::unbounded_channel::<ChangeRecord>();

        // Canonicalize the watch root. Native backends (most notably FSEvents
        // on macOS) report paths against the canonical realpath, so a caller
        // that passes `/var/foo` — a symlink to `/private/var/foo` — must be
        // matched against the resolved target or no filter will ever hit.
        let watch_root =
            std::fs::canonicalize(&self.config.dir).unwrap_or_else(|_| self.config.dir.clone());

        let include = Arc::new(self.config.include.clone());
        let exclude = Arc::new(self.config.exclude.clone());
        let include_empty = self.config.include_empty;
        let watch_root_for_match = watch_root.clone();
        let event_sink = move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                let Some(kind) = ChangeKind::from_event_kind(event.kind) else {
                    return;
                };
                for path in event.paths {
                    // Match against the path relative to the watch root. If
                    // the event somehow surfaces a path outside the root,
                    // reject defensively rather than guess.
                    let Ok(rel) = path.strip_prefix(&watch_root_for_match) else {
                        continue;
                    };
                    if !path_matches(rel, include_empty, &include, &exclude) {
                        continue;
                    }
                    // Receiver drop just means the trigger task exited; in
                    // that case the event is intentionally discarded.
                    drop(tx.send(ChangeRecord {
                        path: path.clone(),
                        kind,
                    }));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "watch trigger received notify error");
            }
        };

        let mut watcher: Box<dyn Watcher + Send> = match &self.backend {
            WatchBackend::Recommended => {
                Box::new(notify::recommended_watcher(event_sink)?) as Box<dyn Watcher + Send>
            }
            WatchBackend::Poll { interval } => {
                let poll = interval.unwrap_or_else(|| Duration::from_millis(200));
                let config = NotifyConfig::default().with_poll_interval(poll);
                Box::new(PollWatcher::new(event_sink, config)?) as Box<dyn Watcher + Send>
            }
        };

        watcher.watch(&watch_root, RecursiveMode::Recursive)?;

        if self.config.per_file {
            self.run_per_file(
                &mut rx,
                cancel,
                self.config.cooldown,
                &path_key,
                &kind_key,
                &timestamp_key,
            )
            .await?;
        } else {
            let cooldown = self
                .config
                .cooldown
                .unwrap_or_else(|| Duration::from_millis(250));
            self.run_batched(&mut rx, cancel, cooldown, &files_key)
                .await?;
        }

        // Drop watcher explicitly so that the `notify` thread exits.
        drop(watcher);
        Ok(())
    }

    async fn run_per_file(
        &self,
        rx: &mut mpsc::UnboundedReceiver<ChangeRecord>,
        cancel: CancellationToken,
        cooldown: Option<Duration>,
        path_key: &MetadataKey,
        kind_key: &MetadataKey,
        timestamp_key: &MetadataKey,
    ) -> Result<(), WatchTriggerError<Q::Error>> {
        let mut debouncer = PathDebouncer::default();
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                maybe_record = rx.recv() => {
                    let Some(record) = maybe_record else {
                        return Ok(());
                    };
                    if !debouncer.admit(&record.path, tokio::time::Instant::now(), cooldown) {
                        continue;
                    }
                    let mut metadata = self.base_metadata.clone();
                    metadata.insert(
                        path_key.clone(),
                        MetadataValue::String(record.path.to_string_lossy().into_owned()),
                    );
                    metadata.insert(
                        kind_key.clone(),
                        MetadataValue::String(record.kind.as_str().to_owned()),
                    );
                    metadata.insert(
                        timestamp_key.clone(),
                        MetadataValue::String(Utc::now().to_rfc3339()),
                    );
                    let signal = Signal::new(metadata);
                    let signal_id = signal.id();
                    self.queue_signal(
                        signal,
                        tracing::info_span!(
                            "iter.trigger.watch.event",
                            iter.trigger.kind = "watch",
                            iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                            iter.signal.id = %signal_id,
                            iter.watch.mode = "per_file",
                            iter.watch.path = %record.path.display(),
                            iter.watch.change.kind = record.kind.as_str(),
                        ),
                    )
                    .await?;
                }
            }
        }
    }

    async fn run_batched(
        &self,
        rx: &mut mpsc::UnboundedReceiver<ChangeRecord>,
        cancel: CancellationToken,
        cooldown: Duration,
        files_key: &MetadataKey,
    ) -> Result<(), WatchTriggerError<Q::Error>> {
        loop {
            // Wait for the first change or cancellation.
            let first = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                maybe = rx.recv() => maybe,
            };
            let Some(first) = first else {
                return Ok(());
            };

            let mut batch: Vec<ChangeRecord> = vec![first];
            let deadline = tokio::time::Instant::now() + cooldown;

            // Drain additional events until cooldown elapses.
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {
                        // Flush whatever we have, then exit.
                        self.flush_batch(batch, files_key).await?;
                        return Ok(());
                    }
                    () = tokio::time::sleep(remaining) => break,
                    maybe = rx.recv() => {
                        match maybe {
                            Some(record) => batch.push(record),
                            None => break,
                        }
                    }
                }
            }

            self.flush_batch(batch, files_key).await?;
        }
    }

    async fn flush_batch(
        &self,
        batch: Vec<ChangeRecord>,
        files_key: &MetadataKey,
    ) -> Result<(), WatchTriggerError<Q::Error>> {
        if batch.is_empty() {
            return Ok(());
        }
        // Deduplicate paths but preserve insertion order.
        let mut seen: std::collections::BTreeSet<std::path::PathBuf> =
            std::collections::BTreeSet::new();
        let mut paths: Vec<String> = Vec::with_capacity(batch.len());
        for record in batch {
            if seen.insert(record.path.clone()) {
                paths.push(record.path.to_string_lossy().into_owned());
            }
        }
        let file_count = paths.len();
        let json = serde_json::to_string(&paths)?;
        let mut metadata = self.base_metadata.clone();
        metadata.insert(files_key.clone(), MetadataValue::String(json));
        let signal = Signal::new(metadata);
        let signal_id = signal.id();
        self.queue_signal(
            signal,
            tracing::info_span!(
                "iter.trigger.watch.batch",
                iter.trigger.kind = "watch",
                iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                iter.signal.id = %signal_id,
                iter.watch.mode = "batch",
                iter.watch.file.count = file_count,
            ),
        )
        .await?;
        Ok(())
    }

    async fn queue_signal(
        &self,
        signal: Signal,
        span: tracing::Span,
    ) -> Result<(), WatchTriggerError<Q::Error>> {
        async move {
            let mut signal = signal;
            iter_core::telemetry::inject_current_context_into_signal(&mut signal);
            self.queue
                .queue(signal, self.priority)
                .await
                .map_err(WatchTriggerError::Queue)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::queue::InMemoryQueue;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::{sleep, timeout};

    fn poll_backend() -> WatchBackend {
        WatchBackend::Poll {
            interval: Some(Duration::from_millis(80)),
        }
    }

    /// Drain everything currently in the queue. Caller must close the queue
    /// first so the dequeue loop terminates.
    async fn drain<Q: Queue>(queue: &Q) -> Vec<Signal> {
        let mut out = Vec::new();
        let dq_cancel = CancellationToken::new();
        while let Ok(Some(s)) = queue.dequeue(dq_cancel.clone()).await {
            out.push(s);
        }
        out
    }

    /// Wait until at least one signal is sitting in the queue, or fail.
    async fn wait_for_first_signal(queue: &InMemoryQueue) {
        timeout(Duration::from_secs(5), async {
            loop {
                if !queue.is_empty().await {
                    return;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("PollWatcher never observed any change");
    }

    #[tokio::test]
    async fn exclude_pattern_filters_out_changes() {
        let tmp = TempDir::new().unwrap();
        let ok_dir = tmp.path().join("ok");
        let skip_dir = tmp.path().join("skip");
        fs::create_dir_all(&ok_dir).unwrap();
        fs::create_dir_all(&skip_dir).unwrap();
        let ok_file = ok_dir.join("a.txt");
        let skip_file = skip_dir.join("b.txt");

        let queue = Arc::new(InMemoryQueue::new());
        let config = WatchConfig::new(
            tmp.path().to_path_buf(),
            &["**/*.txt".to_string()],
            &["skip/**".to_string()],
            true,
            None,
        )
        .unwrap();
        let trigger = WatchTrigger::new(queue.clone(), config).with_backend(poll_backend());

        let cancel = CancellationToken::new();
        let cancel_run = cancel.clone();
        let handle = tokio::spawn(async move { trigger.run(cancel_run).await });

        // Wait for the initial PollWatcher snapshot to settle so the file
        // creates below are observed as separate change events.
        sleep(Duration::from_millis(800)).await;
        fs::write(&ok_file, "ok-1").unwrap();
        fs::write(&skip_file, "skip-1").unwrap();

        wait_for_first_signal(queue.as_ref()).await;
        // Give the watcher several more poll cycles so any (incorrectly
        // admitted) skip/ event has time to arrive before we cancel. With an
        // 80 ms poll interval, 1 s buys 12+ cycles — enough headroom for a
        // loaded CI runner.
        sleep(Duration::from_millis(1000)).await;

        cancel.cancel();
        handle.await.unwrap().unwrap();
        queue.close().await.unwrap();

        let signals = drain(queue.as_ref()).await;
        let path_key = MetadataKey::new("path").unwrap();
        let kind_key = MetadataKey::new("kind").unwrap();
        let timestamp_key = MetadataKey::new("timestamp").unwrap();

        let mut saw_ok = false;
        for s in &signals {
            let MetadataValue::String(path) = s.metadata().get(&path_key).expect("path metadata")
            else {
                panic!("path not string");
            };
            assert!(
                !path.contains("/skip/"),
                "excluded path leaked into signals: {path}"
            );
            // Every emitted signal must carry kind + a parseable RFC3339
            // timestamp.
            assert!(matches!(
                s.metadata().get(&kind_key),
                Some(MetadataValue::String(_))
            ));
            let MetadataValue::String(ts) = s
                .metadata()
                .get(&timestamp_key)
                .expect("timestamp metadata")
            else {
                panic!("timestamp not a string");
            };
            chrono::DateTime::parse_from_rfc3339(ts).expect("rfc3339 timestamp");
            if path.contains("/ok/") {
                saw_ok = true;
            }
        }
        assert!(
            saw_ok,
            "expected at least one ok/ signal, got {}",
            signals.len()
        );
    }
}
