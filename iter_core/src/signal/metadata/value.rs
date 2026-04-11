//! [`MetadataValue`] — scalar value stored under a [`MetadataKey`](super::MetadataKey).

use std::fmt;

use serde::{Deserialize, Serialize};

/// A scalar value stored under a [`MetadataKey`](super::MetadataKey).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    /// UTF-8 string value.
    String(String),
    /// 64-bit signed integer value.
    Integer(i64),
    /// Boolean value.
    Bool(bool),
    /// Explicit null value.
    Null,
}

impl fmt::Display for MetadataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Integer(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Null => f.write_str("null"),
        }
    }
}

impl From<String> for MetadataValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for MetadataValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<i64> for MetadataValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<bool> for MetadataValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
