//! [`CronTrigger`] — fires one signal per cron schedule tick.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use iter_core::{Metadata, MetadataError, MetadataKey, MetadataValue, Priority, Queue, Signal};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// Errors produced by [`CronTrigger`].
#[derive(Debug, Error)]
pub enum CronTriggerError<E: std::error::Error + Send + Sync + 'static> {
    /// The supplied cron expression failed to parse.
    #[error("invalid cron expression: {0}")]
    InvalidExpression(String),

    /// The supplied IANA timezone name failed to parse.
    #[error("invalid timezone: {0}")]
    InvalidTimezone(String),

    /// Forwarded error from the queue backing the trigger.
    #[error("queue error: {0}")]
    Queue(#[source] E),

    /// Construction of an internal metadata key failed. This indicates a
    /// programming error in the trigger itself rather than user input.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),
}

/// A trigger that fires according to a cron schedule.
///
/// Each tick produces one [`Signal`] whose metadata includes a
/// `scheduled_at` field with the RFC 3339 timestamp of the tick. The
/// supplied expression is parsed by the `cron` crate, which accepts both
/// 5-field (standard) and 6/7-field (with seconds and optional year)
/// expressions; this trigger normalises a 5-field expression by prepending
/// `0 ` so that ticks fall on the start of a minute.
pub struct CronTrigger<Q: Queue + ?Sized> {
    queue: Arc<Q>,
    schedule: Schedule,
    base_metadata: Metadata,
    priority: Priority,
    timezone: Tz,
    at_startup: bool,
    catch_up_secs: u64,
    jitter_secs: u64,
    rng_seed: Option<u64>,
    trigger_name: Option<String>,
}

impl<Q: Queue + ?Sized> std::fmt::Debug for CronTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronTrigger")
            .field("schedule", &self.schedule.to_string())
            .field("priority", &self.priority)
            .field("timezone", &self.timezone.name())
            .field("at_startup", &self.at_startup)
            .field("catch_up_secs", &self.catch_up_secs)
            .field("jitter_secs", &self.jitter_secs)
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + ?Sized + 'static> CronTrigger<Q> {
    /// Build a cron trigger publishing to `queue` driven by `expression`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// `expression` may be a 5-field cron string (e.g. `"* * * * *"`) or a
    /// 6/7-field one as accepted by the `cron` crate.
    pub fn new(
        queue: Arc<Q>,
        expression: &str,
    ) -> Result<Self, CronTriggerError<iter_core::queue::QueueError>> {
        let normalized = normalize_expression(expression);
        let schedule = Schedule::from_str(&normalized)
            .map_err(|e| CronTriggerError::InvalidExpression(e.to_string()))?;
        Ok(Self {
            queue,
            schedule,
            base_metadata: Metadata::new(),
            priority: Priority::NORMAL,
            timezone: Tz::UTC,
            at_startup: false,
            catch_up_secs: 0,
            jitter_secs: 0,
            rng_seed: None,
            trigger_name: None,
        })
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

    /// Emit one signal with `startup = true` metadata before entering the
    /// scheduled loop.
    #[must_use]
    pub fn with_at_startup(mut self, at_startup: bool) -> Self {
        self.at_startup = at_startup;
        self
    }

    /// Interpret the cron schedule against `tz` instead of UTC.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_timezone(mut self, tz: Tz) -> Self {
        self.timezone = tz;
        self
    }

    /// Parse and set a timezone from its IANA name.
    ///
    /// # Errors
    /// Returns [`CronTriggerError::InvalidTimezone`] if the name is not a
    /// known IANA zone.
    pub fn try_with_timezone_name(
        mut self,
        name: &str,
    ) -> Result<Self, CronTriggerError<iter_core::queue::QueueError>> {
        let tz: Tz = name
            .parse()
            .map_err(|_| CronTriggerError::InvalidTimezone(name.to_owned()))?;
        self.timezone = tz;
        Ok(self)
    }

    /// On startup, emit a single signal for the most recent missed tick
    /// inside `window`. Set to [`Duration::ZERO`] to disable.
    #[must_use]
    pub fn with_catch_up(mut self, window: Duration) -> Self {
        self.catch_up_secs = window.as_secs();
        self
    }

    /// Sleep for a random amount in `0..=jitter` seconds before each tick
    /// fires, smoothing over coordinated bursts.
    #[must_use]
    pub fn with_jitter(mut self, jitter: Duration) -> Self {
        self.jitter_secs = jitter.as_secs();
        self
    }

    /// Seed the jitter RNG for deterministic tests.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = Some(seed);
        self
    }
}

fn normalize_expression(expr: &str) -> String {
    let parts = expr.split_whitespace().count();
    if parts == 5 {
        format!("0 {expr}")
    } else {
        expr.to_owned()
    }
}

impl<Q: Queue + ?Sized + 'static> CronTrigger<Q> {
    /// Drive the cron trigger until cancellation.
    ///
    /// # Errors
    ///
    /// Returns `CronTriggerError` if metadata construction or queue enqueue fails.
    pub async fn run(
        self,
        cancel: CancellationToken,
    ) -> Result<(), CronTriggerError<iter_core::queue::QueueError>> {
        let scheduled_key = MetadataKey::new("scheduled_at")?;
        let startup_key = MetadataKey::new("startup")?;
        let catch_up_key = MetadataKey::new("catch_up")?;

        if self.catch_up_secs > 0 {
            self.emit_catch_up(&scheduled_key, &catch_up_key).await?;
        }
        if self.at_startup {
            self.emit_startup(&startup_key).await?;
        }

        let mut rng: StdRng = match self.rng_seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_entropy(),
        };

        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let now = Utc::now().with_timezone(&self.timezone);
            let Some(next) = self.schedule.after(&now).next() else {
                return Ok(());
            };
            let wait = next.signed_duration_since(now);
            let std_wait = wait.to_std().unwrap_or(Duration::ZERO);

            if std_wait > Duration::ZERO {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Ok(()),
                    () = tokio::time::sleep(std_wait) => {}
                }
            }

            if self.jitter_secs > 0 {
                let delta = rng.gen_range(0..=self.jitter_secs);
                if delta > 0 {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Ok(()),
                        () = tokio::time::sleep(Duration::from_secs(delta)) => {}
                    }
                }
            }

            if cancel.is_cancelled() {
                return Ok(());
            }

            let mut metadata = self.base_metadata.clone();
            metadata.insert(
                scheduled_key.clone(),
                MetadataValue::String(next.to_rfc3339()),
            );
            let signal = Signal::new(metadata);
            let signal_id = signal.id();
            self.queue_signal(
                signal,
                tracing::info_span!(
                    "iter.trigger.cron.fire",
                    iter.trigger.kind = "cron",
                    iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                    iter.signal.id = %signal_id,
                    iter.trigger.fire.kind = "scheduled",
                    iter.cron.scheduled_at = %next.to_rfc3339(),
                ),
            )
            .await?;

            tracing::trace!(?next, "cron trigger emitted signal");
        }
    }
}

impl<Q: Queue + ?Sized + 'static> CronTrigger<Q> {
    async fn emit_startup(
        &self,
        startup_key: &MetadataKey,
    ) -> Result<(), CronTriggerError<iter_core::queue::QueueError>> {
        let mut metadata = self.base_metadata.clone();
        metadata.insert(startup_key.clone(), MetadataValue::Bool(true));
        let signal = Signal::new(metadata);
        let signal_id = signal.id();
        self.queue_signal(
            signal,
            tracing::info_span!(
                "iter.trigger.cron.fire",
                iter.trigger.kind = "cron",
                iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                iter.signal.id = %signal_id,
                iter.trigger.fire.kind = "startup",
            ),
        )
        .await
    }

    async fn emit_catch_up(
        &self,
        scheduled_key: &MetadataKey,
        catch_up_key: &MetadataKey,
    ) -> Result<(), CronTriggerError<iter_core::queue::QueueError>> {
        let now_utc = Utc::now();
        let window_start = now_utc
            - chrono::Duration::seconds(i64::try_from(self.catch_up_secs).unwrap_or(i64::MAX));
        let window_start_tz = window_start.with_timezone(&self.timezone);
        let now_tz = now_utc.with_timezone(&self.timezone);

        let missed: Option<DateTime<Tz>> = self
            .schedule
            .after(&window_start_tz)
            .take_while(|dt| *dt <= now_tz)
            .last();
        if let Some(tick) = missed {
            let mut metadata = self.base_metadata.clone();
            metadata.insert(
                scheduled_key.clone(),
                MetadataValue::String(tick.to_rfc3339()),
            );
            metadata.insert(catch_up_key.clone(), MetadataValue::Bool(true));
            let signal = Signal::new(metadata);
            let signal_id = signal.id();
            self.queue_signal(
                signal,
                tracing::info_span!(
                    "iter.trigger.cron.fire",
                    iter.trigger.kind = "cron",
                    iter.trigger.name = self.trigger_name.as_deref().unwrap_or(""),
                    iter.signal.id = %signal_id,
                    iter.trigger.fire.kind = "catch_up",
                    iter.cron.scheduled_at = %tick.to_rfc3339(),
                ),
            )
            .await?;
        }
        Ok(())
    }

    async fn queue_signal(
        &self,
        signal: Signal,
        span: tracing::Span,
    ) -> Result<(), CronTriggerError<iter_core::queue::QueueError>> {
        async move {
            let signal = iter_core::telemetry::inject_current_context_into_signal(signal);
            self.queue
                .enqueue(signal, self.priority)
                .await
                .map_err(CronTriggerError::Queue)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::queue::InMemoryQueue;

    #[test]
    fn normalize_5_field_prepends_seconds() {
        assert_eq!(normalize_expression("* * * * *"), "0 * * * * *");
    }

    #[test]
    fn normalize_6_field_passes_through() {
        assert_eq!(normalize_expression("0 * * * * *"), "0 * * * * *");
    }

    #[tokio::test]
    async fn at_startup_emits_startup_metadata() {
        let queue = Arc::new(InMemoryQueue::new());
        let trigger = CronTrigger::new(queue.clone(), "0 0 * * * *")
            .expect("valid cron")
            .with_at_startup(true);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { trigger.run(cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();
        let dq_cancel = CancellationToken::new();
        let signal = queue.dequeue(dq_cancel).await.unwrap().expect("signal");
        let key = MetadataKey::new("startup").unwrap();
        assert_eq!(
            signal.metadata().get(&key),
            Some(&MetadataValue::Bool(true))
        );
    }

    #[tokio::test]
    async fn invalid_timezone_name_errors() {
        let queue = Arc::new(InMemoryQueue::new());
        let err = CronTrigger::new(queue, "0 0 * * * *")
            .unwrap()
            .try_with_timezone_name("Not/AZone")
            .expect_err("should reject");
        assert!(matches!(err, CronTriggerError::InvalidTimezone(_)));
    }

    #[tokio::test]
    async fn jitter_fits_into_range() {
        let queue = Arc::new(InMemoryQueue::new());
        // Every second; jitter 1s; run briefly.
        let trigger = CronTrigger::new(queue.clone(), "* * * * * *")
            .unwrap()
            .with_jitter(Duration::from_secs(1))
            .with_rng_seed(42);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { trigger.run(cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(2500)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();
        // Mostly asserting it runs without panicking when jitter is configured.
    }

    #[tokio::test]
    async fn catch_up_emits_one_missed_tick() {
        // Every minute from the top; window large enough to cover the most
        // recent missed tick.
        let queue = Arc::new(InMemoryQueue::new());
        let trigger = CronTrigger::new(queue.clone(), "0 * * * * *")
            .unwrap()
            .with_catch_up(Duration::from_secs(120));
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { trigger.run(cancel_clone).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();

        let dq_cancel = CancellationToken::new();
        let signal = queue.dequeue(dq_cancel).await.unwrap().expect("signal");
        let catch_up_key = MetadataKey::new("catch_up").unwrap();
        assert_eq!(
            signal.metadata().get(&catch_up_key),
            Some(&MetadataValue::Bool(true))
        );
    }
}
