//! Wire-format envelope for [`Signal`] + [`Priority`] across queue backends.
//!
//! Every backend that ships bytes over the wire — the SQS message body, the
//! shell queue's NDJSON output — uses [`encode_signal`] / [`decode_signal`] so
//! the wire schema is identical and forward-compatible.
//!
//! The format is versioned JSON:
//!
//! ```json
//! {"v": 1, "signal": <Signal as JSON>, "priority": <Priority as integer>}
//! ```
//!
//! Future schema bumps add a new `v` value while keeping older versions
//! decodable.
//!
//! Backends with native priority/attribute slots (SQS message attributes,
//! Service Bus `priority`, etc.) typically also project the priority out of
//! band so it shows up in their native consoles for observability — but the
//! envelope remains the source of truth.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::queue::Priority;
use crate::signal::Signal;

const CURRENT_VERSION: u32 = 1;

/// On-the-wire envelope. Public so backend integration tests can spell it out
/// directly when they bypass [`encode_signal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Format version. Always [`CURRENT_VERSION`] for newly-encoded envelopes.
    pub v: u32,
    /// The signal payload.
    pub signal: Signal,
    /// The priority associated with this signal at enqueue time.
    pub priority: Priority,
}

/// Error decoding an [`Envelope`] from bytes.
#[derive(Debug, Error)]
pub enum EnvelopeError {
    /// JSON parsing failed.
    #[error("envelope decode failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Envelope was syntactically valid JSON but used an unknown schema
    /// version.
    #[error("unsupported envelope version {0}; this iter build understands {CURRENT_VERSION}")]
    UnsupportedVersion(u32),
}

/// Encode a signal + priority into the wire format.
///
/// # Panics
///
/// Never panics: the inner `serde_json::to_vec` cannot fail for these types.
#[must_use]
pub fn encode_signal(signal: &Signal, priority: Priority) -> Vec<u8> {
    let env = Envelope {
        v: CURRENT_VERSION,
        signal: signal.clone(),
        priority,
    };
    serde_json::to_vec(&env).expect("envelope serialization is infallible for this schema")
}

/// Decode a signal + priority from the wire format.
///
/// # Errors
///
/// Returns [`EnvelopeError::Json`] for malformed bytes and
/// [`EnvelopeError::UnsupportedVersion`] when the envelope's `v` field is
/// from a newer iter build than this binary knows about.
pub fn decode_signal(bytes: &[u8]) -> Result<(Signal, Priority), EnvelopeError> {
    let env: Envelope = serde_json::from_slice(bytes)?;
    if env.v != CURRENT_VERSION {
        return Err(EnvelopeError::UnsupportedVersion(env.v));
    }
    Ok((env.signal, env.priority))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

    fn fixture() -> Signal {
        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("workspace").expect("key"),
            MetadataValue::String("alpha".into()),
        );
        Signal::new(metadata)
    }

    #[test]
    fn round_trip_preserves_signal_and_priority() {
        let signal = fixture();
        let bytes = encode_signal(&signal, Priority::HIGH);
        let (back, priority) = decode_signal(&bytes).expect("decode");
        assert_eq!(back, signal);
        assert_eq!(priority, Priority::HIGH);
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let bytes = br#"{"v":99,"signal":{"id":"00000000-0000-0000-0000-000000000000","created_at":"2026-01-01T00:00:00Z","metadata":{}},"priority":50}"#;
        let err = decode_signal(bytes).expect_err("unknown version");
        assert!(matches!(err, EnvelopeError::UnsupportedVersion(99)));
    }

    #[test]
    fn decode_rejects_garbage() {
        let err = decode_signal(b"not json").expect_err("garbage");
        assert!(matches!(err, EnvelopeError::Json(_)));
    }

    #[test]
    fn current_envelope_version_is_one() {
        let signal = fixture();
        let bytes = encode_signal(&signal, Priority::NORMAL);
        let env: Envelope = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(env.v, 1);
    }
}
