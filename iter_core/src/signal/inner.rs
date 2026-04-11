//! [`Signal`] — the unit of work that crosses the [`Queue`](crate::queue::Queue)
//! boundary.
//!
//! A `Signal` carries a [`SignalId`], a creation timestamp, and a [`Metadata`]
//! map that templates can interpolate when rendering a prompt.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::id::SignalId;
use super::kind::SignalKind;
use super::metadata::Metadata;

/// A pure event flowing through the [`Queue`](crate::queue::Queue).
///
/// A `Signal` carries an [`SignalId`], a creation timestamp, a
/// [`SignalKind`] discriminator, and a [`Metadata`] map that templates
/// can interpolate when rendering a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    id: SignalId,
    created_at: DateTime<Utc>,
    #[serde(default)]
    kind: SignalKind,
    metadata: Metadata,
}

impl Signal {
    /// Create a new `Signal` with a freshly generated id and `Utc::now()`
    /// timestamp.
    #[must_use]
    pub fn new(metadata: Metadata) -> Self {
        Self {
            id: SignalId::new(),
            created_at: Utc::now(),
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

    /// Create a termination signal.
    ///
    /// When a runner dequeues a terminate signal it exits gracefully
    /// without invoking the agent.  Terminate signals are enqueued by
    /// triggers (or any external producer) that want the runner to stop.
    #[must_use]
    pub fn terminate() -> Self {
        Self {
            id: SignalId::new(),
            created_at: Utc::now(),
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

    /// Mutably borrow the metadata map.
    pub fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
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
    fn legacy_signal_without_kind_deserializes_as_work() {
        let json = r#"{"id":"019756d7-4e76-7000-8000-000000000001","created_at":"2026-01-01T00:00:00Z","metadata":{}}"#;
        let signal: Signal = serde_json::from_str(json).expect("deserialize");
        assert_eq!(signal.kind(), SignalKind::Work);
    }
}
