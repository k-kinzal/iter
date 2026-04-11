//! Top-level umbrella error type for the `iter_core` crate.
//!
//! Most modules expose their own dedicated error enum (for example
//! [`crate::config::ConfigError`] or [`crate::template::TemplateError`]).
//! The [`Error`] type defined here re-bundles those into a single value
//! that upper layers can return without naming each individual variant.

use crate::config::ConfigError;
use crate::runner::RunnerExitError;
use crate::signal::MetadataError;
use crate::template::TemplateError;

/// Convenience [`Result`] alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Umbrella error covering every fallible operation exported by `iter_core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration loading or parsing failure.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Metadata key validation failure.
    #[error(transparent)]
    Metadata(#[from] MetadataError),

    /// Template compilation or rendering failure.
    #[error(transparent)]
    Template(#[from] TemplateError),

    /// Runner exit error.
    #[error(transparent)]
    Runner(#[from] RunnerExitError),
}
