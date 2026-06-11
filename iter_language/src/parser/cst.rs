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
        guard: Option<CstGuard>,
        /// Literal body of the prompt (triple-string contents are dedented).
        body: String,
        /// Source span of the body literal.
        body_span: Span,
        /// Full span of the section.
        span: Span,
    },
    /// Top-level `on <ident> { ... }`.
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
    /// Nested `on "..." { ... }` routes (used by webhook trigger).
    pub routes: Vec<CstRoute>,
    /// Nested `shell "<cmd>"` actions (used by top-level event handlers).
    pub actions: Vec<CstAction>,
    /// Prompt match arms: `<guard> => <value>` entries (used inside runner
    /// prompt match blocks).
    pub prompt_arms: Vec<CstPromptMatchArm>,
    /// Nested `on <ident> { ... }` event handlers (used inside runner blocks).
    pub event_handlers: Vec<CstEventHandler>,
    /// Full span of the block including braces.
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
    pub guard: Option<CstGuard>,
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

/// A `shell "<command>"` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstAction {
    /// Source span of the `shell` keyword.
    pub keyword_span: Span,
    /// The literal command string.
    pub command: String,
    /// Full span of the action statement.
    pub span: Span,
}

/// Boolean guard expression as captured by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CstGuard {
    /// `metadata.<key> == "<value>"`.
    MetadataEq {
        /// Metadata key being compared.
        key: String,
        /// Literal value compared against.
        value: String,
        /// Span covering the comparison.
        span: Span,
    },
    /// `metadata.<key> != "<value>"`.
    MetadataNeq {
        /// Metadata key being compared.
        key: String,
        /// Literal value compared against.
        value: String,
        /// Span covering the comparison.
        span: Span,
    },
    /// `iteration.<field> [% N] <op> <integer>` numeric comparison. The
    /// optional modulus is captured as part of the same predicate so the
    /// semantic layer can validate `% 0` and `previous_result %` in one
    /// place.
    IterationCmp {
        /// Field name as it appeared in source (validated by the
        /// semantic layer).
        field: String,
        /// Source span of the field name (used to anchor "unknown
        /// field" diagnostics).
        field_span: Span,
        /// Optional `% N` modulus applied to the LHS before comparison.
        modulus: Option<i64>,
        /// Span of the modulus literal when present.
        modulus_span: Option<Span>,
        /// Comparison operator as captured by the parser.
        op: CstCmpOp,
        /// Span of the comparison operator.
        op_span: Span,
        /// Right-hand-side integer literal.
        rhs: i64,
        /// Span of the RHS literal.
        rhs_span: Span,
        /// Span covering the whole comparison.
        span: Span,
    },
    /// `iteration.<field> == "<value>"`. The parser captures the LHS
    /// field name verbatim — only `previous_result` is meaningful here,
    /// but enforcing that is a *semantic* concern so the differential
    /// pest oracle (which only sees the syntactic shape) and the
    /// hand-written parser agree on accept/reject. The semantic layer
    /// rejects any other field with a "string RHS only valid for
    /// `previous_result`" diagnostic, then validates the value against
    /// the closed `"none" | "success" | "errored"` set.
    IterationResultEq {
        /// LHS field name as written (e.g. `"previous_result"`,
        /// `"count"`).
        field: String,
        /// Span of the LHS field identifier.
        field_span: Span,
        /// Literal value compared against.
        value: String,
        /// Span of the RHS string literal.
        value_span: Span,
        /// Span covering the whole comparison.
        span: Span,
    },
    /// `iteration.<field> != "<value>"`. Same field-validation contract
    /// as [`Self::IterationResultEq`].
    IterationResultNeq {
        /// LHS field name as written.
        field: String,
        /// Span of the LHS field identifier.
        field_span: Span,
        /// Literal value compared against.
        value: String,
        /// Span of the RHS string literal.
        value_span: Span,
        /// Span covering the whole comparison.
        span: Span,
    },
    /// Logical conjunction.
    And(Box<CstGuard>, Box<CstGuard>, Span),
    /// Logical disjunction.
    Or(Box<CstGuard>, Box<CstGuard>, Span),
}

/// Comparison operator captured for `iteration.*` numeric predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstCmpOp {
    /// `==`
    Eq,
    /// `!=`
    Neq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CstGuard {
    /// Return the source span associated with this guard.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            CstGuard::MetadataEq { span, .. }
            | CstGuard::MetadataNeq { span, .. }
            | CstGuard::IterationCmp { span, .. }
            | CstGuard::IterationResultEq { span, .. }
            | CstGuard::IterationResultNeq { span, .. }
            | CstGuard::And(_, _, span)
            | CstGuard::Or(_, _, span) => span.clone(),
        }
    }
}
