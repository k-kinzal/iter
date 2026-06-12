//! Prompt templating, guards, and selection.
//!
//! A [`PromptTemplate`] is a handlebars-style string that interpolates values
//! from a [`Signal`](crate::signal::Signal). The supported variables are:
//!
//! * `{{signal.id}}` — UUID v7 string of the signal.
//! * `{{signal.created_at}}` — RFC 3339 / ISO 8601 timestamp.
//! * `{{today}}` — current local date as `YYYY-MM-DD`.
//! * `{{metadata.KEY}}` — value from the signal metadata.
//!
//! Literal `{{` may be escaped by writing `\{{` (backslash + `{{`).
//!
//! [`PromptGuard`] is a boolean AST that examines a signal's metadata; a
//! [`PromptSelector`] combines an ordered list of guarded branches with an
//! optional default template, and hands the runner the rendered
//! [`Prompt`] for each signal.

pub mod error;
pub mod guard;
// Defining module named for the concept it defines — the path echo is deliberate.
#[allow(clippy::module_inception)]
pub mod prompt;
pub mod selector;
pub mod template;

#[cfg(test)]
mod test_helpers;

pub use error::SelectorError;
pub use guard::{CmpOp, IterationField, PromptGuard};
pub use prompt::Prompt;
pub use selector::PromptSelector;
pub use template::PromptTemplate;
