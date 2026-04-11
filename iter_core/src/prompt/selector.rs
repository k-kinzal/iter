//! [`PromptSelector`] — selects and renders the appropriate
//! [`PromptTemplate`] for a given [`Signal`].

use crate::runner::iteration::IterationContext;
use crate::signal::Signal;

use super::error::SelectorError;
use super::guard::PromptGuard;
use super::inner::Prompt;
use super::template::PromptTemplate;

/// Selects and renders the appropriate [`PromptTemplate`] for a given
/// [`Signal`].
///
/// A selector holds an ordered list of guarded branches plus at most one
/// unguarded default template. [`PromptSelector::render`] walks the
/// branches in source order and returns the first one whose guard
/// matches; if none match it falls back to the default; if there is no
/// default it returns [`SelectorError::NoMatchingPrompt`].
///
/// The default is a fallback regardless of where it appeared in the
/// source Iterfile: guarded branches are always tried first.
#[derive(Debug, Clone)]
pub struct PromptSelector {
    branches: Vec<(PromptGuard, PromptTemplate)>,
    default: Option<PromptTemplate>,
}

impl PromptSelector {
    /// Build a selector from an ordered list of guarded branches plus an
    /// optional default. Callers that only have a single unguarded
    /// template should prefer [`PromptSelector::single`].
    #[must_use]
    pub fn new(
        branches: Vec<(PromptGuard, PromptTemplate)>,
        default: Option<PromptTemplate>,
    ) -> Self {
        Self { branches, default }
    }

    /// Build a selector containing a single unguarded template. This is
    /// the trivial form used when an Iterfile only declares one
    /// `prompt` block.
    #[must_use]
    pub fn single(template: PromptTemplate) -> Self {
        Self {
            branches: Vec::new(),
            default: Some(template),
        }
    }

    /// Borrow the ordered list of guarded branches.
    #[must_use]
    pub fn branches(&self) -> &[(PromptGuard, PromptTemplate)] {
        &self.branches
    }

    /// Borrow the default template, if one was configured.
    #[must_use]
    pub fn default_template(&self) -> Option<&PromptTemplate> {
        self.default.as_ref()
    }

    /// Pick the matching template for `signal` and `iteration` and render
    /// it.
    ///
    /// # Errors
    ///
    /// * [`SelectorError::NoMatchingPrompt`] when no guard matches and no
    ///   default template was supplied.
    /// * [`SelectorError::Template`] when the chosen template fails to
    ///   render against `signal` / `iteration`.
    pub fn render(
        &self,
        signal: &Signal,
        iteration: &IterationContext,
    ) -> Result<Prompt, SelectorError> {
        for (guard, template) in &self.branches {
            if guard.matches(signal, iteration) {
                return Ok(template.render(signal, iteration)?);
            }
        }
        match &self.default {
            Some(template) => Ok(template.render(signal, iteration)?),
            None => Err(SelectorError::NoMatchingPrompt),
        }
    }
}

impl From<PromptTemplate> for PromptSelector {
    fn from(template: PromptTemplate) -> Self {
        Self::single(template)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::test_helpers::{guard_kind_eq, signal_with, signal_with_kind};
    use crate::signal::metadata::Metadata;

    fn prompt_template(source: &str) -> PromptTemplate {
        PromptTemplate::new(source).expect("compile")
    }

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    #[test]
    fn selector_single_renders_unconditionally() {
        let selector = PromptSelector::single(prompt_template("hi"));
        let signal = signal_with(Metadata::new());
        assert_eq!(
            selector.render(&signal, &iter_ctx()).unwrap().as_str(),
            "hi"
        );
    }

    #[test]
    fn selector_picks_first_matching_branch() {
        let selector = PromptSelector::new(
            vec![
                (guard_kind_eq("issue"), prompt_template("handle issue")),
                (guard_kind_eq("ci_fix"), prompt_template("fix ci")),
            ],
            None,
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("issue"), &iter_ctx())
                .unwrap()
                .as_str(),
            "handle issue"
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("ci_fix"), &iter_ctx())
                .unwrap()
                .as_str(),
            "fix ci"
        );
    }

    #[test]
    fn selector_falls_back_to_default_when_no_branch_matches() {
        let selector = PromptSelector::new(
            vec![(guard_kind_eq("issue"), prompt_template("issue path"))],
            Some(prompt_template("default path")),
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("other"), &iter_ctx())
                .unwrap()
                .as_str(),
            "default path"
        );
    }

    #[test]
    fn selector_errors_when_no_branch_matches_and_no_default() {
        let selector = PromptSelector::new(
            vec![(guard_kind_eq("issue"), prompt_template("issue path"))],
            None,
        );
        let err = selector
            .render(&signal_with_kind("other"), &iter_ctx())
            .unwrap_err();
        assert!(matches!(err, SelectorError::NoMatchingPrompt));
    }

    #[test]
    fn selector_renders_metadata_substitution_in_chosen_branch() {
        let selector = PromptSelector::new(
            vec![(
                guard_kind_eq("issue"),
                prompt_template("handle {{metadata.kind}}"),
            )],
            None,
        );
        let signal = signal_with_kind("issue");
        assert_eq!(
            selector.render(&signal, &iter_ctx()).unwrap().as_str(),
            "handle issue"
        );
    }

    #[test]
    fn selector_source_order_trumps_default_position() {
        // A default that appears BEFORE guarded branches in the
        // branch/default construction must still only be used as a
        // fallback, not as a first match.
        let selector = PromptSelector::new(
            vec![(guard_kind_eq("urgent"), prompt_template("urgent"))],
            Some(prompt_template("fallback")),
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("urgent"), &iter_ctx())
                .unwrap()
                .as_str(),
            "urgent"
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("other"), &iter_ctx())
                .unwrap()
                .as_str(),
            "fallback"
        );
    }
}
