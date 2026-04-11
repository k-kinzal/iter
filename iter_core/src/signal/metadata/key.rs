//! [`MetadataKey`] — validated key for the [`Metadata`](super::Metadata) map.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::error::MetadataError;

/// A validated key for the [`Metadata`](super::Metadata) map.
///
/// Keys must be non-empty and contain only ASCII alphanumerics, `_`, `-`, or
/// `.`. This is the minimal set required for templates such as
/// `{{metadata.foo.bar}}` to be unambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MetadataKey(String);

impl MetadataKey {
    /// Create a new key, returning [`MetadataError::InvalidKey`] when the
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// supplied string is empty or contains a forbidden character.
    pub fn new(key: impl Into<String>) -> Result<Self, MetadataError> {
        let key = key.into();
        if key.is_empty() {
            return Err(MetadataError::InvalidKey(key));
        }
        if !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(MetadataError::InvalidKey(key));
        }
        Ok(Self(key))
    }

    /// Borrow the key as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MetadataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for MetadataKey {
    type Err = MetadataError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for MetadataKey {
    type Error = MetadataError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MetadataKey> for String {
    fn from(value: MetadataKey) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_rejects_empty() {
        assert!(MetadataKey::new("").is_err());
    }

    #[test]
    fn key_validation_rejects_invalid_chars() {
        assert!(MetadataKey::new("oops space").is_err());
        assert!(MetadataKey::new("こんにちは").is_err());
    }

    #[test]
    fn key_validation_accepts_alphanumerics_and_separators() {
        for k in ["foo", "foo_bar", "foo-bar", "foo.bar", "FOO123"] {
            MetadataKey::new(k).unwrap_or_else(|_| panic!("expected {k} to validate"));
        }
    }
}
