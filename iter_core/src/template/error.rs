//! [`TemplateError`] — failures from compiling or rendering a [`Template`](super::Template).

/// Errors produced while compiling or rendering a [`Template`](super::Template).
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// The template source was syntactically invalid (e.g. unbalanced `{{ }}`).
    #[error("invalid template syntax: {0}")]
    InvalidSyntax(String),

    /// The template referenced a variable that has no binding in the context.
    #[error("unknown template variable: {0}")]
    UnknownVariable(String),

    /// A generic render failure unrelated to missing variables (helper
    /// errors, serialization failures, etc.).
    #[error("template render failed: {0}")]
    Render(String),
}
