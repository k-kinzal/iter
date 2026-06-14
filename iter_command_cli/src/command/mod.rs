//! [`CommandTrigger`] — polls a shell command and emits one signal per
//! extracted record.

mod dedupe;
mod error;
mod extract;

pub use error::CommandTriggerError;
pub use extract::ExtractMode;

/// Behaviour when the polled command exits with a non-zero status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnError {
    /// Log a warning and continue polling (default).
    #[default]
    Continue,
    /// Stop the trigger and return an error.
    Abort,
    /// Silently skip the failed poll (no warning, no signals emitted).
    Skip,
}

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use iter_core::{Metadata, MetadataKey, MetadataValue, Priority, Queue, Signal};
use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use dedupe::canonicalize;
use extract::{json_to_metadata_value, value_to_string};

/// A trigger that polls a shell command on a fixed interval.
///
/// Each poll runs the command, applies the configured [`ExtractMode`], and
/// emits one [`Signal`] per record. When `deduplicate` is enabled, records
/// observed in earlier polls are not re-emitted; the dedupe set is reset when
/// the trigger is restarted.
pub struct CommandTrigger<Q: Queue + ?Sized> {
    queue: Arc<Q>,
    command: String,
    shell: String,
    extract: ExtractMode,
    poll: Duration,
    deduplicate: bool,
    on_error: OnError,
    base_metadata: Metadata,
    priority: Priority,
    trigger_name: Option<String>,
}

impl<Q: Queue + ?Sized> std::fmt::Debug for CommandTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandTrigger")
            .field("command", &self.command)
            .field("shell", &self.shell)
            .field("extract", &self.extract)
            .field("poll", &self.poll)
            .field("deduplicate", &self.deduplicate)
            .field("on_error", &self.on_error)
            .field("priority", &self.priority)
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + ?Sized + 'static> CommandTrigger<Q> {
    /// Build a command trigger.
    ///
    /// `shell` is the program/argv-prefix used to interpret `command`, e.g.
    /// `"sh -c"` or `"bash -c"`. The trailing `-c` (or equivalent flag) is
    /// required so that the supplied command string is interpreted by the
    /// shell.
    #[must_use]
    pub fn new(
        queue: Arc<Q>,
        command: impl Into<String>,
        shell: impl Into<String>,
        extract: ExtractMode,
        poll: Duration,
    ) -> Self {
        Self {
            queue,
            command: command.into(),
            shell: shell.into(),
            extract,
            poll,
            deduplicate: false,
            on_error: OnError::Continue,
            base_metadata: Metadata::new(),
            priority: Priority::NORMAL,
            trigger_name: None,
        }
    }

    /// Enable or disable cross-poll deduplication.
    #[must_use]
    pub fn with_deduplicate(mut self, dedupe: bool) -> Self {
        self.deduplicate = dedupe;
        self
    }

    /// Choose how the trigger reacts to non-zero command exits.
    #[must_use]
    pub fn with_on_error(mut self, on_error: OnError) -> Self {
        self.on_error = on_error;
        self
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
    /// Returns `CommandTriggerError` if command execution or queue enqueue fails.
    pub async fn run(
        self,
        cancel: CancellationToken,
    ) -> Result<(), CommandTriggerError<iter_core::queue::QueueError>> {
        let mut seen: HashSet<String> = HashSet::new();
        let regex = if let ExtractMode::Regex(pat) = &self.extract {
            Some(regex::Regex::new(pat).map_err(|e| CommandTriggerError::Regex(e.to_string()))?)
        } else {
            None
        };

        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }

            let should_continue = async {
                // Run the command.
                let (success, output) = match self.run_command(&cancel).await {
                    Ok(Some(out)) => out,
                    Ok(None) => return Ok(false),
                    Err(err) => return Err(err),
                };

                if !success {
                    match self.on_error {
                        OnError::Continue => {
                            tracing::warn!(
                                "command trigger subprocess exited non-zero; continuing"
                            );
                        }
                        OnError::Abort => {
                            return Err(CommandTriggerError::Aborted(output));
                        }
                        OnError::Skip => {
                            tracing::trace!("command trigger subprocess exited non-zero; skipping");
                            return Ok(true);
                        }
                    }
                }

                let records = self.extract_records(&output, regex.as_ref())?;

                for record in records {
                    let canonical = canonicalize(&record);
                    if self.deduplicate && !seen.insert(canonical) {
                        continue;
                    }
                    let metadata = self.build_metadata(&record)?;
                    let signal = Signal::new(metadata);
                    let signal_id = signal.id();
                    self.queue_signal(
                        signal,
                        tracing::info_span!(
                            "iter.trigger.command.emit",
                            iter.trigger.kind = "command",
                            iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                            iter.signal.id = %signal_id,
                        ),
                    )
                    .await?;
                }

                Ok(true)
            }
            .instrument(tracing::info_span!(
                "iter.trigger.command.poll",
                iter.trigger.kind = "command",
                iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
            ))
            .await?;

            if !should_continue {
                return Ok(());
            }

            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }

    async fn run_command(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<(bool, String)>, CommandTriggerError<iter_core::queue::QueueError>> {
        let mut parts = self.shell.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| CommandTriggerError::InvalidShell(self.shell.clone()))?;
        let args: Vec<&str> = parts.collect();

        let mut cmd = Command::new(program);
        cmd.args(&args)
            .arg(&self.command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let output_fut = cmd.output();
        let output = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(None),
            res = output_fut => res?,
        };

        let success = output.status.success();
        if !success {
            tracing::trace!(
                status = ?output.status,
                stderr = %String::from_utf8_lossy(&output.stderr),
                "command trigger subprocess exited non-zero"
            );
        }

        Ok(Some((
            success,
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )))
    }

    fn build_metadata(
        &self,
        record: &Value,
    ) -> Result<Metadata, CommandTriggerError<iter_core::queue::QueueError>> {
        let mut metadata = self.base_metadata.clone();
        match record {
            Value::Object(map) => {
                for (k, v) in map {
                    let key = MetadataKey::new(k.as_str())?;
                    metadata.insert(key, json_to_metadata_value(v));
                }
            }
            other @ (Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)) => {
                let key = MetadataKey::new("value")?;
                metadata.insert(key, MetadataValue::String(value_to_string(other)));
            }
        }
        Ok(metadata)
    }

    async fn queue_signal(
        &self,
        signal: Signal,
        span: tracing::Span,
    ) -> Result<(), CommandTriggerError<iter_core::queue::QueueError>> {
        async move {
            let signal = iter_core::telemetry::inject_current_context_into_signal(signal);
            self.queue
                .enqueue(signal, self.priority)
                .await
                .map_err(CommandTriggerError::Queue)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::queue::InMemoryQueue;

    #[tokio::test]
    async fn on_error_abort_returns_err() {
        let queue = Arc::new(InMemoryQueue::new());
        let trigger = CommandTrigger::new(
            queue.clone(),
            "exit 1".to_string(),
            "sh -c",
            ExtractMode::Lines,
            Duration::from_secs(60),
        )
        .with_on_error(OnError::Abort);
        let cancel = CancellationToken::new();
        let err = trigger.run(cancel).await.expect_err("abort should error");
        assert!(matches!(err, CommandTriggerError::Aborted(_)));
    }

    #[tokio::test]
    async fn on_error_skip_keeps_running() {
        let queue = Arc::new(InMemoryQueue::new());
        let trigger = CommandTrigger::new(
            queue.clone(),
            "exit 1".to_string(),
            "sh -c",
            ExtractMode::Lines,
            Duration::from_millis(10),
        )
        .with_on_error(OnError::Skip);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { trigger.run(cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        handle
            .await
            .unwrap()
            .expect("skip should not produce an error");
    }
}
