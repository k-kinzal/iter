//! [`MetadataError`] — failures surfaced while constructing or mutating
//! [`Metadata`](super::Metadata).

/// Errors emitted while constructing or mutating [`Metadata`](super::Metadata).
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// The supplied key was empty or contained an invalid character.
    #[error("invalid metadata key: {0}")]
    InvalidKey(String),
}
