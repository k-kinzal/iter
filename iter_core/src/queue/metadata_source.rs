//! [`MetadataSource`] — a send-time value that is either a fixed literal or
//! read from a [`Signal`]'s metadata.
//!
//! This is the runtime twin of the language's `MetadataSource`: the language
//! *describes* the value (`"lit"` or `from_metadata("key")`), core *resolves*
//! it against a signal at send time. It performs **no** templating — it either
//! returns the literal or looks up exactly one metadata key.
//!
//! It lives at the queue boundary (not under a single backend) because more
//! than one backend reads it: today the SQS producer uses it for the FIFO
//! `MessageGroupId` / `MessageDeduplicationId`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::signal::Signal;

/// A send-time value: a literal, or a single metadata key read off the signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataSource {
    /// Raw literal value.
    Literal(String),
    /// Look up the named metadata key on the signal at runtime.
    FromMetadata(String),
}

/// The metadata key referenced by [`MetadataSource::FromMetadata`] was absent
/// on the signal at resolve time.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("metadata key `{key}` referenced by from_metadata() is absent on the signal")]
pub struct MissingMetadata {
    /// Metadata key that was missing.
    pub key: String,
}

impl MetadataSource {
    /// Resolve the source against a signal's metadata.
    ///
    /// # Errors
    ///
    /// Returns [`MissingMetadata`] when a [`FromMetadata`](Self::FromMetadata)
    /// source references a key the signal does not carry.
    pub fn resolve(&self, signal: &Signal) -> Result<String, MissingMetadata> {
        match self {
            Self::Literal(s) => Ok(s.clone()),
            Self::FromMetadata(key) => signal
                .metadata()
                .get_str(key)
                .map(ToString::to_string)
                .ok_or_else(|| MissingMetadata { key: key.clone() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{Metadata, MetadataKey, MetadataValue};

    fn signal_with(key: &str, value: &str) -> Signal {
        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new(key).expect("valid key"),
            MetadataValue::String(value.into()),
        );
        Signal::new(metadata)
    }

    #[test]
    fn literal_resolves_to_itself() {
        let s = MetadataSource::Literal("static".into());
        assert_eq!(s.resolve(&signal_with("k", "v")).unwrap(), "static");
    }

    #[test]
    fn from_metadata_reads_the_key() {
        let s = MetadataSource::FromMetadata("workspace".into());
        let sig = signal_with("workspace", "alpha");
        assert_eq!(s.resolve(&sig).unwrap(), "alpha");
    }

    #[test]
    fn from_metadata_missing_key_errors() {
        let s = MetadataSource::FromMetadata("missing".into());
        let err = s.resolve(&signal_with("present", "v")).expect_err("missing");
        assert_eq!(err.key, "missing");
    }
}
