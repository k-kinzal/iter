//! Adoption-token validation failures.
//!
//! Used by [`super::adopt::AdoptError::CorruptToken`].

/// Why a bootstrap token failed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum TokenCorruptKind {
    /// Body length was not exactly 32 hex characters.
    WrongLength {
        /// Actual length (in characters).
        actual: usize,
    },
    /// Body contained a non-hex character. The contained `String` is the
    /// offending sample (truncated).
    NonHex(String),
    /// Body contained upper-case hex characters; the on-disk form is
    /// canonicalised to lower-case.
    UpperCase,
}
