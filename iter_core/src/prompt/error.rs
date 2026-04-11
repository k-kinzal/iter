//! [`SelectorError`] — failures surfaced by [`PromptSelector`](super::PromptSelector).

use crate::template::TemplateError;

/// Errors produced by [`PromptSelector::render`](super::PromptSelector::render).
#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    /// No guarded prompt branch matched the signal and no default branch
    /// was configured, so the [`PromptSelector`](super::PromptSelector)
    /// cannot produce a prompt.
    #[error("no prompt branch matched this signal")]
    NoMatchingPrompt,

    /// Rendering the selected template failed.
    #[error(transparent)]
    Template(#[from] TemplateError),
}
