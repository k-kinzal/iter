//! [`Template`] — Handlebars-backed templating shared by prompts, shell
//! event handlers, and webhook metadata rendering.
//!
//! A [`Template`] is compiled once and rendered many times. The underlying
//! renderer is [`handlebars`] in strict mode with HTML escaping disabled, so a
//! reference to a variable absent from the render context surfaces a
//! [`TemplateError::UnknownVariable`] rather than rendering empty — the
//! run-time backstop behind the analysis-time position checks. Rendered text
//! is emitted verbatim (no `&amp;` / `&lt;` rewrites).
//!
//! The template grammar is whatever Handlebars supports:
//! `{{path.to.value}}` interpolation, `{{#if …}}` blocks, and so on. The
//! iter DSL uses a flat subset (`{{signal.id}}`, `{{metadata.*}}`, …) but
//! richer contexts such as webhook JSON payloads benefit from the full
//! syntax.

pub mod context;
pub mod error;

pub use context::{IterationRenderContext, RunnerRenderContext, SignalContext};
pub use error::TemplateError;

use std::fmt;
use std::hash::{Hash, Hasher};

use handlebars::{Handlebars, RenderErrorReason, TemplateError as HbsTemplateError};
use serde::{Deserialize, Serialize};

const TEMPLATE_KEY: &str = "t";

/// A compiled Handlebars template.
///
/// Build one with [`Template::compile`], then reuse it across many
/// [`Template::render`] calls. [`Template`] owns its compiled form so no
/// locking is required to render concurrently from `&Template`.
pub struct Template {
    registry: Handlebars<'static>,
    source: String,
}

impl Template {
    /// Compile `source` into a reusable template.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] when the source is not a
    /// valid Handlebars template (unbalanced `{{ … }}`, empty expressions,
    /// etc.).
    pub fn compile(source: impl Into<String>) -> Result<Self, TemplateError> {
        let source = source.into();
        let mut registry = Handlebars::new();
        registry.set_strict_mode(true);
        registry.register_escape_fn(handlebars::no_escape);
        registry
            .register_template_string(TEMPLATE_KEY, &source)
            .map_err(|e| map_compile_error(&e))?;
        Ok(Self { registry, source })
    }

    /// Borrow the original source string.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Render against a serializable context.
    ///
    /// Strict mode is on: a reference to a variable that the context does
    /// not carry surfaces an error instead of rendering empty.
    ///
    /// # Errors
    ///
    /// * [`TemplateError::UnknownVariable`] — the template references a
    ///   path absent from the context (e.g. a metadata key the signal does
    ///   not carry).
    /// * [`TemplateError::Render`] — any other render failure (helper
    ///   errors, context serialization issues, …).
    pub fn render<T: Serialize>(&self, ctx: &T) -> Result<String, TemplateError> {
        self.registry
            .render(TEMPLATE_KEY, ctx)
            .map_err(|e| map_render_error(&e))
    }
}

fn map_compile_error(err: &HbsTemplateError) -> TemplateError {
    TemplateError::InvalidSyntax(err.to_string())
}

fn map_render_error(err: &handlebars::RenderError) -> TemplateError {
    match err.reason() {
        RenderErrorReason::MissingVariable(Some(path)) => {
            TemplateError::UnknownVariable(path.clone())
        }
        RenderErrorReason::MissingVariable(None) => TemplateError::UnknownVariable(String::new()),
        _ => TemplateError::Render(err.to_string()),
    }
}

impl fmt::Debug for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Template")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Clone for Template {
    fn clone(&self) -> Self {
        // Re-compile from source: the source has already compiled once,
        // so this is expected to succeed. Using `expect` rather than
        // bubbling the error keeps `Clone` infallible.
        Self::compile(self.source.clone()).expect("source previously compiled")
    }
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for Template {}

impl Hash for Template {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.source.hash(state);
    }
}

impl Serialize for Template {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.source.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = String::deserialize(deserializer)?;
        Self::compile(source).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compile_rejects_unterminated_braces() {
        let err = Template::compile("hello {{signal.id").unwrap_err();
        assert!(matches!(err, TemplateError::InvalidSyntax(_)));
    }

    #[test]
    fn compile_rejects_empty_expression() {
        let err = Template::compile("hello {{}}").unwrap_err();
        assert!(matches!(err, TemplateError::InvalidSyntax(_)));
    }

    #[test]
    fn render_errors_on_missing_top_level() {
        let tpl = Template::compile("{{nope}}").unwrap();
        let err = tpl.render(&json!({})).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownVariable(_)));
    }

    #[test]
    fn render_errors_on_missing_nested_path() {
        let tpl = Template::compile("{{metadata.missing}}").unwrap();
        let err = tpl
            .render(&json!({"metadata": {"other": "x"}}))
            .unwrap_err();
        assert!(matches!(err, TemplateError::UnknownVariable(_)));
    }

    #[test]
    fn render_no_html_escape() {
        // The default handlebars escape function turns `<&>` into
        // `&lt;&amp;&gt;`. We install `no_escape`, so the string flows
        // through unchanged.
        let tpl = Template::compile("{{v}}").unwrap();
        let out = tpl.render(&json!({"v": "<&>"})).unwrap();
        assert_eq!(out, "<&>");
    }

    #[test]
    fn render_escapes_double_braces() {
        // `\{{` is the handlebars escape for a literal `{{`.
        let tpl = Template::compile("literal \\{{x}}").unwrap();
        let out = tpl.render(&json!({"x": "ignored"})).unwrap();
        assert_eq!(out, "literal {{x}}");
    }

    #[test]
    fn render_whitespace_inside_expression_is_ignored() {
        let tpl = Template::compile("{{   metadata.x   }}").unwrap();
        let out = tpl.render(&json!({"metadata": {"x": 42}})).unwrap();
        assert_eq!(out, "42");
    }

    #[test]
    fn template_clone_recompiles_from_source() {
        let tpl = Template::compile("hi {{name}}").unwrap();
        let cloned = tpl.clone();
        let out = cloned.render(&json!({"name": "you"})).unwrap();
        assert_eq!(out, "hi you");
        assert_eq!(tpl.source(), cloned.source());
    }

    #[test]
    fn template_eq_compares_source() {
        let a = Template::compile("x={{x}}").unwrap();
        let b = Template::compile("x={{x}}").unwrap();
        let c = Template::compile("y={{y}}").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn template_serialize_is_transparent_to_source() {
        let tpl = Template::compile("hello {{name}}").unwrap();
        let json = serde_json::to_string(&tpl).unwrap();
        assert_eq!(json, "\"hello {{name}}\"");
    }

    #[test]
    fn template_deserialize_compiles_source() {
        let tpl: Template = serde_json::from_str("\"hi {{name}}\"").unwrap();
        let out = tpl.render(&json!({"name": "there"})).unwrap();
        assert_eq!(out, "hi there");
    }

    #[test]
    fn template_deserialize_rejects_invalid_source() {
        let err = serde_json::from_str::<Template>("\"hello {{\"").unwrap_err();
        // Error message should mention the invalid syntax.
        let msg = err.to_string();
        assert!(
            msg.contains("invalid template syntax") || msg.contains("InvalidSyntax"),
            "unexpected error: {msg}"
        );
    }
}
