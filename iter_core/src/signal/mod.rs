//! [`Signal`] — the unit of work flowing through the
//! [`Queue`](crate::queue::Queue) — and its associated identifier and
//! metadata types.
//!
//! A `Signal` carries four pieces of information:
//!
//! * a [`SignalId`] (UUID v7) that is both unique and time-ordered;
//! * a creation timestamp so downstream consumers can reason about age;
//! * a [`SignalKind`] discriminator separating ordinary work from control
//!   signals such as termination — an open, `#[non_exhaustive]` set that new
//!   runner/trigger control semantics extend with additional typed variants;
//! * a [`Metadata`] map of validated keys to scalar values that prompt
//!   templates and guards read from.

pub mod defaults;
pub mod id;
pub mod kind;
pub mod metadata;
// Defining module named for the concept it defines — the path echo is deliberate.
#[allow(clippy::module_inception)]
pub mod signal;

pub use defaults::{MetadataPairError, base_metadata, parse_metadata_pair, parse_metadata_pairs};
pub use id::SignalId;
pub use kind::SignalKind;
pub use metadata::{Metadata, MetadataError, MetadataKey, MetadataValue};
pub use signal::Signal;
