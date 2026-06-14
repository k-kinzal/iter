//! [`Signal`] — the unit of work that crosses the [`Queue`](crate::queue::Queue)
//! boundary.
//!
//! A `Signal` carries a [`SignalId`], a creation timestamp, and a [`Metadata`]
//! map that templates can interpolate when rendering a prompt.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::id::SignalId;
use super::kind::SignalKind;
use super::metadata::{Metadata, MetadataKey, MetadataValue};
use crate::time::{Clock, IdSource, SystemClock, SystemIdSource};

/// A pure event flowing through the [`Queue`](crate::queue::Queue).
///
/// A `Signal` carries an [`SignalId`], a creation timestamp, a
/// [`SignalKind`] discriminator, and a [`Metadata`] map that templates
/// can interpolate when rendering a prompt.
///
/// A `Signal` is an **immutable unit of work**: once constructed it never
/// changes. It is a value object identified by its [`SignalId`], carried by
/// reference — and shared across an iteration's lifecycle events via
/// [`SharedSignal`](crate::runner::SharedSignal) — for the whole bracket. To
/// derive a signal that carries additional metadata (for example a trigger
/// injecting trace context before publishing), construct a new one with
/// [`Signal::with_metadata_value`] rather than mutating in place; there is no
/// mutable metadata accessor by design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    id: SignalId,
    created_at: DateTime<Utc>,
    #[serde(default)]
    kind: SignalKind,
    metadata: Metadata,
}

impl Signal {
    /// Create a new `Signal` with the system clock and id source.
    #[must_use]
    pub fn new(metadata: Metadata) -> Self {
        Self::new_with_sources(metadata, &SystemClock, &SystemIdSource)
    }

    /// Create a new `Signal` with injected time and id sources.
    #[must_use]
    pub fn new_with_sources(
        metadata: Metadata,
        clock: &dyn Clock,
        id_source: &dyn IdSource,
    ) -> Self {
        Self {
            id: id_source.new_id(),
            created_at: clock.now(),
            kind: SignalKind::Work,
            metadata,
        }
    }

    /// Create a synthesized `Signal` carrying empty metadata.
    ///
    /// Used by the [`Runner`](crate::Runner) when `behavior = loop` is
    /// configured and no real signal is available — the runner needs *some*
    /// signal to drive the next runner iteration, and a freshly minted
    /// empty-metadata one is the canonical placeholder.
    ///
    /// Prompt templates that interpolate `{{metadata.foo}}` will fail to
    /// render against a synthesized signal, which is the desired
    /// behaviour: the operator either authors a template that does not
    /// depend on per-signal metadata or pairs the loop with a real trigger.
    #[must_use]
    pub fn synthesized() -> Self {
        Self::new(Metadata::new())
    }

    /// Create a synthesized `Signal` with injected time and id sources.
    #[must_use]
    pub fn synthesized_with_sources(clock: &dyn Clock, id_source: &dyn IdSource) -> Self {
        Self::new_with_sources(Metadata::new(), clock, id_source)
    }

    /// Create a termination signal.
    ///
    /// When a runner dequeues a terminate signal it exits gracefully
    /// without invoking the agent.  Terminate signals are enqueued by
    /// triggers (or any external producer) that want the runner to stop.
    #[must_use]
    pub fn terminate() -> Self {
        Self::terminate_with_sources(&SystemClock, &SystemIdSource)
    }

    /// Create a termination signal with injected time and id sources.
    #[must_use]
    pub fn terminate_with_sources(clock: &dyn Clock, id_source: &dyn IdSource) -> Self {
        Self {
            id: id_source.new_id(),
            created_at: clock.now(),
            kind: SignalKind::Terminate,
            metadata: Metadata::new(),
        }
    }

    /// Create a `Signal` with explicit identifier and creation time.
    ///
    /// Useful when re-hydrating a signal from a persistent queue.
    #[must_use]
    pub fn with_id(
        id: SignalId,
        created_at: DateTime<Utc>,
        kind: SignalKind,
        metadata: Metadata,
    ) -> Self {
        Self {
            id,
            created_at,
            kind,
            metadata,
        }
    }

    /// Identifier of this signal.
    #[must_use]
    pub fn id(&self) -> SignalId {
        self.id
    }

    /// Wall-clock instant the signal was created.
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// The kind of this signal (work or terminate).
    #[must_use]
    pub fn kind(&self) -> SignalKind {
        self.kind
    }

    /// Returns `true` when this is a termination signal.
    #[must_use]
    pub fn is_terminate(&self) -> bool {
        self.kind == SignalKind::Terminate
    }

    /// Borrow the metadata map.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Derive a new `Signal` that additionally carries `key` → `value` in its
    /// metadata, preserving this signal's id, creation time, and kind.
    ///
    /// The derived signal keeps the **same** [`SignalId`] — it is the same
    /// unit of work with enriched metadata, not a new one. Because the builder
    /// consumes `self`, the caller holds the derived signal as a replacement
    /// for the original, never a same-id sibling.
    ///
    /// A `Signal` is immutable after construction (see the type docs); this
    /// consuming builder is the supported way to add metadata, keeping the
    /// value-object contract intact. An existing entry for `key` is replaced.
    /// Triggers use it to inject trace context onto a freshly minted signal
    /// before enqueueing it.
    #[must_use]
    pub fn with_metadata_value(mut self, key: MetadataKey, value: MetadataValue) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::metadata::{MetadataKey, MetadataValue};

    #[test]
    fn signal_serializes_roundtrip() {
        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("user").expect("key"),
            MetadataValue::String("alice".into()),
        );
        let signal = Signal::new(metadata);
        let json = serde_json::to_string(&signal).expect("serialize");
        let back: Signal = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(signal, back);
    }

    #[test]
    fn work_signal_is_default_kind() {
        let signal = Signal::new(Metadata::new());
        assert_eq!(signal.kind(), SignalKind::Work);
        assert!(!signal.is_terminate());
    }

    #[test]
    fn terminate_signal_has_terminate_kind() {
        let signal = Signal::terminate();
        assert_eq!(signal.kind(), SignalKind::Terminate);
        assert!(signal.is_terminate());
    }

    #[test]
    fn terminate_signal_serializes_roundtrip() {
        let signal = Signal::terminate();
        let json = serde_json::to_string(&signal).expect("serialize");
        let back: Signal = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(signal, back);
        assert_eq!(back.kind(), SignalKind::Terminate);
    }

    #[test]
    fn metadata_changes_produce_a_new_signal_leaving_the_original_intact() {
        // `Signal` is immutable: the only public way to add metadata is to
        // construct a new signal. This pins that contract — there is no
        // `metadata_mut`-style accessor to mutate a signal in place.
        let original = Signal::new(Metadata::new());
        let key = MetadataKey::new("traceparent").expect("key");

        let derived = original
            .clone()
            .with_metadata_value(key.clone(), MetadataValue::String("ctx".into()));

        // The original is untouched...
        assert!(original.metadata().get(&key).is_none());
        // ...while the derived signal carries the new entry but keeps the
        // identity, creation time, and kind of the signal it was derived from.
        assert_eq!(derived.id(), original.id());
        assert_eq!(derived.created_at(), original.created_at());
        assert_eq!(derived.kind(), original.kind());
        assert!(matches!(
            derived.metadata().get(&key),
            Some(MetadataValue::String(_)),
        ));
    }

    #[test]
    fn legacy_signal_without_kind_deserializes_as_work() {
        let json = r#"{"id":"019756d7-4e76-7000-8000-000000000001","created_at":"2026-01-01T00:00:00Z","metadata":{}}"#;
        let signal: Signal = serde_json::from_str(json).expect("deserialize");
        assert_eq!(signal.kind(), SignalKind::Work);
    }
}
