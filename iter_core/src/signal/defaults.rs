//! Signal defaults — the `KEY=VALUE` metadata parsing every Signal source
//! shares when stamping its base [`Metadata`] onto each published Signal.
//!
//! This is the one home for "turn the operator's `--metadata KEY=VALUE`
//! entries into a [`Metadata`] map". It is paired with
//! [`Priority::from_keyword`](crate::queue::Priority::from_keyword), the
//! canonical priority-keyword mapping, to make up the two halves of a Signal
//! source's *signal defaults* (the priority and base metadata stamped before
//! per-event enrichment).
//!
//! Deliberately **clap-free**: the per-binary `--metadata` flag (and its
//! `clap::Args` wrapper) stays in each trigger CLI; only the parsing concept
//! lives here so the five binaries do not re-spell it.

use thiserror::Error;

use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

/// Error parsing a `KEY=VALUE` metadata entry.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetadataPairError {
    /// The entry did not contain a `=` separator.
    #[error("--metadata expects KEY=VALUE, got `{0}`")]
    MissingEquals(String),
    /// The key was rejected by [`MetadataKey::new`].
    #[error("invalid metadata key `{0}`")]
    InvalidKey(String),
}

/// Parse one `KEY=VALUE` entry into a metadata key and its string value.
///
/// The value may itself contain `=`; only the first `=` separates key from
/// value.
///
/// # Errors
///
/// Returns [`MetadataPairError`] if the entry has no `=` or the key is not a
/// valid [`MetadataKey`].
pub fn parse_metadata_pair(entry: &str) -> Result<(MetadataKey, String), MetadataPairError> {
    let (k, v) = entry
        .split_once('=')
        .ok_or_else(|| MetadataPairError::MissingEquals(entry.to_owned()))?;
    let key = MetadataKey::new(k).map_err(|_| MetadataPairError::InvalidKey(k.to_owned()))?;
    Ok((key, v.to_owned()))
}

/// Parse repeated `KEY=VALUE` entries into ordered key/value pairs, preserving
/// input order.
///
/// # Errors
///
/// Returns [`MetadataPairError`] for the first malformed entry.
pub fn parse_metadata_pairs(
    entries: &[String],
) -> Result<Vec<(MetadataKey, String)>, MetadataPairError> {
    entries.iter().map(|e| parse_metadata_pair(e)).collect()
}

/// Build a [`Metadata`] map from repeated `KEY=VALUE` entries.
///
/// Later entries with the same key overwrite earlier ones (the
/// [`Metadata::insert`] semantics).
///
/// # Errors
///
/// Returns [`MetadataPairError`] for the first malformed entry.
pub fn base_metadata(entries: &[String]) -> Result<Metadata, MetadataPairError> {
    let mut out = Metadata::new();
    for (key, value) in parse_metadata_pairs(entries)? {
        out.insert(key, MetadataValue::String(value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pairs_in_order() {
        let pairs = parse_metadata_pairs(&["source=manual".into(), "tag=alpha".into()])
            .expect("pairs");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0.as_str(), "source");
        assert_eq!(pairs[0].1, "manual");
        assert_eq!(pairs[1].0.as_str(), "tag");
    }

    #[test]
    fn value_may_contain_equals() {
        let (key, value) = parse_metadata_pair("expr=a=b=c").expect("pair");
        assert_eq!(key.as_str(), "expr");
        assert_eq!(value, "a=b=c");
    }

    #[test]
    fn missing_equals_is_an_error() {
        assert_eq!(
            parse_metadata_pair("no-eq-here"),
            Err(MetadataPairError::MissingEquals("no-eq-here".into()))
        );
    }

    #[test]
    fn invalid_key_is_an_error() {
        assert!(matches!(
            parse_metadata_pair("not a key=v"),
            Err(MetadataPairError::InvalidKey(_))
        ));
    }

    #[test]
    fn base_metadata_builds_a_map() {
        let meta = base_metadata(&["source=manual".into()]).expect("metadata");
        let key = MetadataKey::new("source").expect("key");
        assert_eq!(meta.get(&key), Some(&MetadataValue::String("manual".into())));
    }
}
