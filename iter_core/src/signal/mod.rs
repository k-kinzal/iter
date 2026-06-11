//! [`Signal`] — the unit of work flowing through the
//! [`Queue`](crate::queue::Queue) — and its associated identifier and
//! metadata types.
//!
//! A `Signal` carries three pieces of information:
//!
//! * a [`SignalId`] (UUID v7) that is both unique and time-ordered;
//! * a creation timestamp so downstream consumers can reason about age;
//! * a [`Metadata`] map of validated keys to scalar values that prompt
//!   templates and guards read from.

pub mod defaults;
pub mod id;
pub mod inner;
pub mod kind;
pub mod metadata;

pub use defaults::{MetadataPairError, base_metadata, parse_metadata_pair, parse_metadata_pairs};
pub use id::SignalId;
pub use inner::Signal;
pub use kind::SignalKind;
pub use metadata::{Metadata, MetadataError, MetadataKey, MetadataValue};
