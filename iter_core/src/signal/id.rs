//! [`SignalId`] — the time-ordered identifier attached to every [`Signal`](super::Signal).
//!
//! The underlying representation is a UUID v7, so lexicographic ordering of
//! `SignalId` matches creation order, which is convenient for queue
//! implementations that want FIFO semantics within a priority bucket.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time::{IdSource, SystemIdSource};

/// Time-ordered identifier (UUID v7) attached to every [`Signal`](super::Signal).
///
/// Because the underlying value is a UUID v7, lexicographic ordering of
/// `SignalId` matches creation order, which is convenient for queue
/// implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SignalId(Uuid);

impl SignalId {
    /// Generate a new time-ordered `SignalId`.
    #[must_use]
    pub fn new() -> Self {
        IdSource::new_id(&SystemIdSource)
    }

    /// Wrap an existing UUID as a `SignalId`.
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// Return the wrapped UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for SignalId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SignalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for SignalId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for SignalId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<SignalId> for Uuid {
    fn from(value: SignalId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_id_is_unique() {
        let a = SignalId::new();
        let b = SignalId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn signal_ids_are_time_ordered() {
        // UUID v7 IDs minted in succession must be monotonically
        // non-decreasing in their lexicographic / Ord representation.
        let mut prev = SignalId::new();
        for _ in 0..50 {
            let next = SignalId::new();
            assert!(next >= prev, "expected v7 ordering: {prev:?} <= {next:?}");
            prev = next;
        }
    }

    #[test]
    fn signal_id_display_roundtrips_through_from_str() {
        let id = SignalId::new();
        let text = id.to_string();
        let parsed: SignalId = text.parse().expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn signal_id_serializes_as_string() {
        let id = SignalId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        let back: SignalId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }
}
