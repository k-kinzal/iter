//! [`Metadata`] map and its key/value types.
//!
//! Metadata is the structured payload carried on a
//! [`Signal`](crate::signal::Signal). It uses a [`BTreeMap`] so iteration
//! order is stable, which keeps prompt rendering deterministic.

pub mod error;
pub mod key;
pub mod value;

pub use error::MetadataError;
pub use key::MetadataKey;
pub use value::MetadataValue;

use std::collections::BTreeMap;
use std::collections::btree_map;

use serde::{Deserialize, Serialize};

/// Ordered key-value store carried on a [`Signal`](crate::signal::Signal).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Metadata(BTreeMap<MetadataKey, MetadataValue>);

impl Metadata {
    /// Create an empty metadata map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a key-value pair, returning the previous value if any.
    pub fn insert(&mut self, key: MetadataKey, value: MetadataValue) -> Option<MetadataValue> {
        self.0.insert(key, value)
    }

    /// Look up a value by key.
    #[must_use]
    pub fn get(&self, key: &MetadataKey) -> Option<&MetadataValue> {
        self.0.get(key)
    }

    /// Look up a value by raw string key.
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&MetadataValue> {
        self.0
            .iter()
            .find_map(|(k, v)| (k.as_str() == key).then_some(v))
    }

    /// Iterate over all entries in key order.
    pub fn iter(&self) -> btree_map::Iter<'_, MetadataKey, MetadataValue> {
        self.0.iter()
    }

    /// Number of entries in the map.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when the map contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(MetadataKey, MetadataValue)> for Metadata {
    fn from_iter<I: IntoIterator<Item = (MetadataKey, MetadataValue)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for Metadata {
    type Item = (MetadataKey, MetadataValue);
    type IntoIter = btree_map::IntoIter<MetadataKey, MetadataValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Metadata {
    type Item = (&'a MetadataKey, &'a MetadataValue);
    type IntoIter = btree_map::Iter<'a, MetadataKey, MetadataValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_and_iter() {
        let mut m = Metadata::new();
        m.insert(MetadataKey::new("a").unwrap(), MetadataValue::Integer(1));
        m.insert(MetadataKey::new("b").unwrap(), MetadataValue::Bool(true));
        m.insert(MetadataKey::new("c").unwrap(), MetadataValue::Null);

        assert_eq!(m.len(), 3);
        assert!(!m.is_empty());
        assert_eq!(
            m.get(&MetadataKey::new("a").unwrap()),
            Some(&MetadataValue::Integer(1))
        );
        let collected: Vec<_> = m.iter().map(|(k, _)| k.as_str().to_owned()).collect();
        assert_eq!(collected, vec!["a", "b", "c"]);
    }

    #[test]
    fn metadata_serializes_roundtrip() {
        let mut m = Metadata::new();
        m.insert(
            MetadataKey::new("name").unwrap(),
            MetadataValue::String("alice".into()),
        );
        m.insert(
            MetadataKey::new("count").unwrap(),
            MetadataValue::Integer(5),
        );
        m.insert(MetadataKey::new("ok").unwrap(), MetadataValue::Bool(false));

        let json = serde_json::to_string(&m).expect("serialize");
        let back: Metadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    #[test]
    fn metadata_from_iter_collects() {
        let m: Metadata = [
            (MetadataKey::new("x").unwrap(), MetadataValue::from(1_i64)),
            (MetadataKey::new("y").unwrap(), MetadataValue::from("two")),
        ]
        .into_iter()
        .collect();
        assert_eq!(m.len(), 2);
    }
}
