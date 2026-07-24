//! `prompt` declaration AST and shared priority keywords.

use super::Expr;

/// A `prompt [when <expr>] "<body>"` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDef {
    /// Optional `when` guard expression.
    pub guard: Option<Expr>,
    /// Raw template body. Placeholders such as `{{metadata.foo}}` are kept
    /// verbatim; they are resolved when the prompt is rendered, not during
    /// analysis.
    pub body: String,
}

/// A named top-level prompt definition: `prompt as <name> "<body>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedPrompt {
    /// User-facing identifier for this prompt template.
    pub name: String,
    /// Raw template body (may contain `{{...}}` placeholders).
    pub body: String,
}

/// Prompt expression inside a runner block — determines which prompt to
/// use on each iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptExpr {
    /// Single prompt, no guards: `prompt = "text"` or `prompt = name`.
    Single(PromptValue),
    /// Match expression: `prompt { guard => value, ..., _ => default }`.
    Match {
        /// Guarded arms evaluated top to bottom.
        arms: Vec<PromptArm>,
        /// Required default arm (`_ => value`).
        default: PromptValue,
    },
}

/// One arm of a prompt match expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptArm {
    /// Guard condition that must be true for this arm to fire.
    pub guard: Expr,
    /// Value selected when the guard is true.
    pub value: PromptValue,
}

/// A prompt value — either an inline string or a reference to a named
/// top-level `prompt as <name>` definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptValue {
    /// Inline string literal (may contain template placeholders).
    Inline(String),
    /// Reference to a named top-level `prompt as <name>` definition.
    Ref(String),
}

/// Standard priority keywords accepted in webhook routes and elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityKeyword {
    /// Lowest priority.
    Low,
    /// Default priority.
    Normal,
    /// Higher than normal.
    High,
    /// Reserved for incidents that must preempt other work.
    Critical,
}
