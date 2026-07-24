//! Concrete Syntax Tree (CST) types produced by the parser.
//!
//! The CST is intentionally generic: each top-level section is captured as a
//! [`CstSection`] tuple so that domain dispatch is a semantic concern, not a
//! grammar one. The types in this module are part of the public grammar
//! contract alongside [`crate::GRAMMAR_VERSION`].

use crate::ast::Span;

/// Top-level node of the concrete syntax tree produced by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstFile {
    /// Top-level sections in source order.
    pub sections: Vec<CstSection>,
}

/// A top-level section of a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstSection {
    /// `queue <kind> [as <alias>] [{ ... }]`, `workspace <kind> [as <alias>] { ... }`, etc.
    ///
    /// The Iterfile grammar uses `<keyword> [<kind>] [as <alias>] { ... }`.
    /// The compose.iter grammar reuses the same CST node with `kind`
    /// carrying the section name and `kind2` carrying the kind:
    /// `<keyword> <name> [<kind2>] { ... }`. Disambiguation between the two
    /// shapes is the semantic layer's job, not the parser's.
    Block {
        /// The leading keyword (`queue`, `workspace`, `agent`, `trigger`, `runner`, `service`).
        keyword: String,
        /// Source span of [`Self::Block::keyword`].
        keyword_span: Span,
        /// First identifier following the keyword. Iterfile semantics treat
        /// this as the kind; compose.iter treats it as the section name.
        kind: Option<CstIdent>,
        /// Optional second identifier. compose.iter uses this to carry the
        /// kind (`queue main file { ... }`); Iterfile semantic rejects it.
        kind2: Option<CstIdent>,
        /// Optional `as <name>` alias. Iterfile uses this to name a
        /// definition: `agent claude as primary { ... }`.
        alias: Option<CstIdent>,
        /// Optional brace-delimited body.
        body: Option<CstBlock>,
        /// Full span of the section.
        span: Span,
    },
    /// `prompt [when <expr>] "<body>"` (old) or `prompt as <name> "<body>"` (new).
    Prompt {
        /// Source span of the `prompt` keyword.
        keyword_span: Span,
        /// Optional `as <name>` for named prompt definitions.
        name: Option<CstIdent>,
        /// Optional `when` guard (old syntax).
        guard: Option<CstExpr>,
        /// Literal body of the prompt (triple-string contents are dedented).
        body: String,
        /// Source span of the body literal.
        body_span: Span,
        /// Full span of the section.
        span: Span,
    },
    /// Top-level `on <ident> { ... }` (Compose Hook syntax; rejected for Iterfiles).
    On {
        /// Source span of the `on` keyword.
        keyword_span: Span,
        /// Event name identifier.
        event: CstIdent,
        /// Body block.
        body: CstBlock,
        /// Full span of the section.
        span: Span,
    },
}

/// An identifier captured during parsing, with its span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstIdent {
    /// Identifier text as it appeared in source.
    pub name: String,
    /// Source span.
    pub span: Span,
}

/// A `{ ... }` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstBlock {
    /// Field assignments such as `port = 8080`.
    pub fields: Vec<CstField>,
    /// Nested `condition <kind> as <name> { ... }` declarations.
    pub conditions: Vec<CstCondition>,
    /// Nested `on "..." { ... }` routes (used by webhook trigger).
    pub routes: Vec<CstRoute>,
    /// Nested `shell "<cmd>"` actions (used by top-level event handlers).
    pub actions: Vec<CstAction>,
    /// Nested `capture <name> { ... }` declarations used by block-form
    /// shell actions.
    pub captures: Vec<CstCapture>,
    /// Prompt match arms: `<guard> => <value>` entries (used inside runner
    /// prompt match blocks).
    pub prompt_arms: Vec<CstPromptMatchArm>,
    /// Nested `on <ident> { ... }` event handlers (used inside runner blocks).
    pub event_handlers: Vec<CstEventHandler>,
    /// Full span of the block including braces.
    pub span: Span,
}

/// A named stream capture nested inside a block-form shell action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstCapture {
    /// Capture name, exposed below `var.<name>`.
    pub name: CstIdent,
    /// Capture-specific fields.
    pub body: CstBlock,
    /// Span covering the complete declaration.
    pub span: Span,
}

/// A named completion condition declaration inside a `completion` block.
///
/// The parser records this shape generically; the semantic layer decides
/// whether the containing block is a runner completion declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstCondition {
    /// Condition kind (`iterations`, `shell`, `elapsed`, or `deadline`).
    pub kind: CstIdent,
    /// User-facing condition name following `as`.
    pub name: CstIdent,
    /// Condition-specific fields.
    pub body: CstBlock,
    /// Span covering the complete declaration.
    pub span: Span,
}

/// A `name = value` (or `name { ... }`) entry inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstField {
    /// Field name identifier.
    pub name: CstIdent,
    /// Field value.
    pub value: CstValue,
    /// Span covering the whole field.
    pub span: Span,
}

/// A literal or composite value on the right-hand side of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstValue {
    /// String literal.
    String(String, Span),
    /// Integer literal.
    Integer(i64, Span),
    /// Duration literal, normalised to seconds.
    Duration(i64, Span),
    /// Boolean literal.
    Bool(bool, Span),
    /// `null` literal — the absence of a value. Used in compose overrides to
    /// remove a definition (e.g. `trigger_name = null` disables a trigger).
    Null(Span),
    /// Bareword identifier value.
    Ident(String, Span),
    /// Heterogeneous list of values.
    List(Vec<CstValue>, Span),
    /// Nested block.
    Block(CstBlock),
    /// Function-call form, e.g. `env("VAR")`.
    Call {
        /// Callee name.
        name: String,
        /// Argument list in source order.
        args: Vec<CstValue>,
        /// Span covering the whole call expression.
        span: Span,
    },
}

impl CstValue {
    /// Return the source span associated with this value.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            CstValue::String(_, s)
            | CstValue::Integer(_, s)
            | CstValue::Duration(_, s)
            | CstValue::Bool(_, s)
            | CstValue::Null(s)
            | CstValue::Ident(_, s)
            | CstValue::List(_, s) => s.clone(),
            CstValue::Block(b) => b.span.clone(),
            CstValue::Call { span, .. } => span.clone(),
        }
    }
}

/// A nested `on <ident> { <actions> }` event handler inside a block
/// (e.g. runner body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstEventHandler {
    /// Event name identifier.
    pub event: CstIdent,
    /// Optional `when <expr>` condition.
    pub condition: Option<CstExpr>,
    /// Body block containing actions.
    pub body: CstBlock,
    /// Full span of the event handler.
    pub span: Span,
}

/// A `<guard> => <value>` arm inside a prompt match block. The default
/// arm uses `_` as the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstPromptMatchArm {
    /// Guard expression (or `None` for the `_` wildcard default arm).
    pub guard: Option<CstExpr>,
    /// Value — either a string literal or a bareword identifier reference.
    pub value: CstValue,
    /// Full span of the arm.
    pub span: Span,
}

/// A nested `on "<pattern>" [when "<expr>"] { ... }` webhook route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstRoute {
    /// Event-pattern string literal.
    pub event_pattern: String,
    /// Optional raw `when` guard string.
    pub when: Option<String>,
    /// Span of the `when` guard string literal (when present), so analysis
    /// can point diagnostics at the guard rather than the whole route.
    pub when_span: Option<Span>,
    /// Body block.
    pub body: CstBlock,
    /// Full span of the route.
    pub span: Span,
}

/// A lifecycle-hook action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstAction {
    /// Source span of the action keyword (`shell` or `enqueue`).
    pub keyword_span: Span,
    /// Concrete action body.
    pub body: CstActionBody,
    /// Full span of the action statement.
    pub span: Span,
}

/// Concrete lifecycle-hook action forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstActionBody {
    /// Backward-compatible `shell "<script>"` form.
    Shorthand {
        /// Literal shell script.
        script: String,
        /// Span of the script literal.
        script_span: Span,
    },
    /// Expanded `shell { script = "..."; capture ... }` form.
    Block(CstBlock),
    /// `enqueue { target = <queue-name> metadata ... priority = ... }`.
    Enqueue(CstBlock),
}

/// Common expression as captured by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstExpr {
    /// Scalar literal in the common expression grammar.
    Literal {
        /// Literal value.
        value: CstExprLiteral,
        /// Source span.
        span: Span,
    },
    /// Rooted path in the common expression grammar.
    Path {
        /// Root identifier.
        root: CstIdent,
        /// Traversal segments.
        segments: Vec<CstPathSegment>,
        /// Source span.
        span: Span,
    },
    /// Binary operation in the common expression grammar.
    Binary {
        /// Left operand.
        lhs: Box<CstExpr>,
        /// Operator.
        op: CstBinaryOp,
        /// Operator source span.
        op_span: Span,
        /// Right operand.
        rhs: Box<CstExpr>,
        /// Full source span.
        span: Span,
    },
}

/// Backwards-compatible name for [`CstExpr`].
pub type CstGuard = CstExpr;

/// Scalar literal in a [`CstExpr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstExprLiteral {
    /// String literal.
    String(String),
    /// Integer literal.
    Integer(i64),
    /// Boolean literal.
    Bool(bool),
    /// Null literal.
    Null,
}

/// One traversal segment in a [`CstExpr::Path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstPathSegment {
    /// Object field.
    Field(CstIdent),
    /// Array index plus source span.
    Index(usize, Span),
}

/// Binary expression operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstBinaryOp {
    /// Boolean OR.
    Or,
    /// Boolean AND.
    And,
    /// Equality.
    Eq,
    /// Inequality.
    Neq,
    /// Less-than.
    Lt,
    /// Less-than-or-equal.
    Le,
    /// Greater-than.
    Gt,
    /// Greater-than-or-equal.
    Ge,
    /// Integer remainder.
    Mod,
}

impl CstExpr {
    /// Return the source span associated with this expression.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            CstExpr::Literal { span, .. }
            | CstExpr::Path { span, .. }
            | CstExpr::Binary { span, .. } => span.clone(),
        }
    }
}
