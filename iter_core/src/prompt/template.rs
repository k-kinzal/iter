//! [`PromptTemplate`] — a handlebars-style template that renders to a
//! [`Prompt`] by interpolating values from a [`Signal`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::runner::iteration::IterationContext;
use crate::signal::Signal;
use crate::template::{RenderContext, Template, TemplateError};

use super::inner::Prompt;

/// A handlebars-style prompt template.
///
/// See the [module documentation](crate::prompt) for the supported
/// substitution syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PromptTemplate {
    template: Template,
}

impl PromptTemplate {
    /// Build a new template from its source string.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] if the source cannot be
    /// compiled.
    pub fn new(source: impl Into<String>) -> Result<Self, TemplateError> {
        Ok(Self {
            template: Template::compile(source)?,
        })
    }

    /// Borrow the underlying template source.
    #[must_use]
    pub fn source(&self) -> &str {
        self.template.source()
    }

    /// Render the template into a [`Prompt`] using values from `signal`
    /// and the runner's `iteration` snapshot.
    ///
    /// # Errors
    ///
    /// Any [`TemplateError`] surfaced by the underlying Handlebars
    /// renderer — most commonly [`TemplateError::UnknownVariable`] when
    /// the template references a missing metadata key.
    pub fn render(
        &self,
        signal: &Signal,
        iteration: &IterationContext,
    ) -> Result<Prompt, TemplateError> {
        let context = RenderContext::new(signal, iteration);
        self.template.render(&context).map(Prompt::from)
    }
}

impl fmt::Display for PromptTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.template.source())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::test_helpers::signal_with;
    use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    #[test]
    fn renders_literal_text_unchanged() {
        let template = PromptTemplate::new("hello world").expect("compile");
        let prompt = template
            .render(&signal_with(Metadata::new()), &iter_ctx())
            .expect("render");
        assert_eq!(prompt.as_str(), "hello world");
    }

    #[test]
    fn renders_metadata_variable() {
        let mut meta = Metadata::new();
        meta.insert(
            MetadataKey::new("name").unwrap(),
            MetadataValue::String("alice".into()),
        );
        let signal = signal_with(meta);
        let template = PromptTemplate::new("Hi, {{metadata.name}}!").expect("compile");
        assert_eq!(
            template.render(&signal, &iter_ctx()).unwrap().as_str(),
            "Hi, alice!"
        );
    }

    #[test]
    fn renders_signal_id_and_created_at() {
        let signal = signal_with(Metadata::new());
        let template =
            PromptTemplate::new("id={{signal.id}} ts={{signal.created_at}}").expect("compile");
        let rendered = template.render(&signal, &iter_ctx()).unwrap();
        assert!(rendered.as_str().contains(&signal.id().to_string()));
        assert!(
            rendered
                .as_str()
                .contains(&signal.created_at().to_rfc3339())
        );
    }

    #[test]
    fn renders_today_as_yyyy_mm_dd() {
        let signal = signal_with(Metadata::new());
        let template = PromptTemplate::new("today is {{today}}").expect("compile");
        let rendered = template.render(&signal, &iter_ctx()).unwrap().into_string();
        let date = rendered.trim_start_matches("today is ");
        assert_eq!(date.len(), 10, "expected YYYY-MM-DD, got {date:?}");
        assert_eq!(date.chars().nth(4), Some('-'));
        assert_eq!(date.chars().nth(7), Some('-'));
    }

    #[test]
    fn unknown_variable_errors() {
        let signal = signal_with(Metadata::new());
        let template = PromptTemplate::new("{{nope}}").expect("compile");
        let err = template.render(&signal, &iter_ctx()).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownVariable(_)));
    }

    #[test]
    fn unknown_metadata_key_errors() {
        let signal = signal_with(Metadata::new());
        let template = PromptTemplate::new("{{metadata.missing}}").expect("compile");
        let err = template.render(&signal, &iter_ctx()).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownVariable(_)));
    }

    #[test]
    fn unterminated_expression_errors_at_compile() {
        let err = PromptTemplate::new("hello {{signal.id").unwrap_err();
        assert!(matches!(err, TemplateError::InvalidSyntax(_)));
    }

    #[test]
    fn empty_expression_errors_at_compile() {
        let err = PromptTemplate::new("hello {{}}").unwrap_err();
        assert!(matches!(err, TemplateError::InvalidSyntax(_)));
    }

    #[test]
    fn escapes_double_braces() {
        let signal = signal_with(Metadata::new());
        let template = PromptTemplate::new("literal \\{{signal.id}}").expect("compile");
        let rendered = template.render(&signal, &iter_ctx()).unwrap();
        assert_eq!(rendered.as_str(), "literal {{signal.id}}");
    }

    #[test]
    fn whitespace_inside_expression_is_ignored() {
        let mut meta = Metadata::new();
        meta.insert(MetadataKey::new("x").unwrap(), MetadataValue::Integer(42));
        let signal = signal_with(meta);
        let template = PromptTemplate::new("{{   metadata.x   }}").expect("compile");
        assert_eq!(
            template.render(&signal, &iter_ctx()).unwrap().as_str(),
            "42"
        );
    }

    #[test]
    fn integer_and_bool_metadata_render() {
        let mut meta = Metadata::new();
        meta.insert(MetadataKey::new("n").unwrap(), MetadataValue::Integer(7));
        meta.insert(MetadataKey::new("b").unwrap(), MetadataValue::Bool(true));
        let signal = signal_with(meta);
        let template = PromptTemplate::new("{{metadata.n}}/{{metadata.b}}").expect("compile");
        assert_eq!(
            template.render(&signal, &iter_ctx()).unwrap().as_str(),
            "7/true"
        );
    }

    #[test]
    fn null_metadata_renders_empty() {
        let mut meta = Metadata::new();
        meta.insert(MetadataKey::new("n").unwrap(), MetadataValue::Null);
        let signal = signal_with(meta);
        let template = PromptTemplate::new("[{{metadata.n}}]").expect("compile");
        assert_eq!(
            template.render(&signal, &iter_ctx()).unwrap().as_str(),
            "[]"
        );
    }

    #[test]
    fn renders_iteration_count() {
        let signal = signal_with(Metadata::new());
        let iter = IterationContext::for_count(7);
        let template = PromptTemplate::new("turn {{iteration.count}}").expect("compile");
        assert_eq!(template.render(&signal, &iter).unwrap().as_str(), "turn 7");
    }

    #[test]
    fn renders_iteration_previous_result() {
        let signal = signal_with(Metadata::new());
        let iter = IterationContext::for_count(1);
        let template = PromptTemplate::new("prev {{iteration.previous_result}}").expect("compile");
        assert_eq!(
            template.render(&signal, &iter).unwrap().as_str(),
            "prev none"
        );
    }
}
