//! Concrete Syntax Tree (CST) types produced by the parser.
//!
//! The CST is intentionally generic: each top-level section is captured as a
//! [`RawSection`] tuple so that domain dispatch is a semantic concern, not a
//! grammar one. The types in this module are part of the public grammar
//! contract alongside [`crate::GRAMMAR_VERSION`].

use crate::ast::Span;

/// Root of the concrete syntax tree produced by [`super::parse_tokens`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFile {
    /// Top-level sections in source order.
    pub sections: Vec<RawSection>,
}

/// A top-level section of a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSection {
    /// `queue <kind> [{ ... }]`, `workspace <kind> { ... }`, etc.
    ///
    /// The Iterfile grammar uses `<keyword> [<kind>] { ... }` (`kind2` is
    /// always `None`). The compose.iter grammar reuses the same CST node
    /// with `kind` carrying the section name and `kind2` carrying the kind:
    /// `<keyword> <name> [<kind2>] { ... }`. Disambiguation between the two
    /// shapes is the semantic layer's job, not the parser's.
    Block {
        /// The leading keyword (`queue`, `workspace`, `agent`, `trigger`, `runner`, `service`).
        keyword: String,
        /// Source span of [`Self::Block::keyword`].
        keyword_span: Span,
        /// First identifier following the keyword. Iterfile semantics treat
        /// this as the kind; compose.iter treats it as the section name.
        kind: Option<RawIdent>,
        /// Optional second identifier. compose.iter uses this to carry the
        /// kind (`queue main file { ... }`); Iterfile semantic rejects it.
        kind2: Option<RawIdent>,
        /// Optional brace-delimited body.
        body: Option<RawBlock>,
        /// Full span of the section.
        span: Span,
    },
    /// `prompt [when <expr>] "<body>"`.
    Prompt {
        /// Source span of the `prompt` keyword.
        keyword_span: Span,
        /// Optional `when` guard.
        guard: Option<RawGuard>,
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
        event: RawIdent,
        /// Body block.
        body: RawBlock,
        /// Full span of the section.
        span: Span,
    },
}

/// An identifier captured during parsing, with its span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdent {
    /// Identifier text as it appeared in source.
    pub name: String,
    /// Source span.
    pub span: Span,
}

/// A `{ ... }` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    /// Field assignments such as `port = 8080`.
    pub fields: Vec<RawField>,
    /// Nested `on "..." { ... }` routes (used by webhook trigger).
    pub routes: Vec<RawRoute>,
    /// Nested `shell "<cmd>"` actions (used by top-level event handlers).
    pub actions: Vec<RawAction>,
    /// Full span of the block including braces.
    pub span: Span,
}

/// A `name = value` (or `name { ... }`) entry inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawField {
    /// Field name identifier.
    pub name: RawIdent,
    /// Field value.
    pub value: RawValue,
    /// Span covering the whole field.
    pub span: Span,
}

/// A literal or composite value on the right-hand side of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawValue {
    /// String literal.
    String(String, Span),
    /// Integer literal.
    Integer(i64, Span),
    /// Duration literal, normalised to seconds.
    Duration(i64, Span),
    /// Boolean literal.
    Bool(bool, Span),
    /// Bareword identifier value.
    Ident(String, Span),
    /// Heterogeneous list of values.
    List(Vec<RawValue>, Span),
    /// Nested block.
    Block(RawBlock),
    /// Function-call form, e.g. `env("VAR")`.
    Call {
        /// Callee name.
        name: String,
        /// Argument list in source order.
        args: Vec<RawValue>,
        /// Span covering the whole call expression.
        span: Span,
    },
}

impl RawValue {
    /// Return the source span associated with this value.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            RawValue::String(_, s)
            | RawValue::Integer(_, s)
            | RawValue::Duration(_, s)
            | RawValue::Bool(_, s)
            | RawValue::Ident(_, s)
            | RawValue::List(_, s) => s.clone(),
            RawValue::Block(b) => b.span.clone(),
            RawValue::Call { span, .. } => span.clone(),
        }
    }
}

/// A nested `on "<pattern>" [when "<expr>"] { ... }` webhook route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRoute {
    /// Event-pattern string literal.
    pub event_pattern: String,
    /// Optional raw `when` guard string.
    pub when: Option<String>,
    /// Body block.
    pub body: RawBlock,
    /// Full span of the route.
    pub span: Span,
}

/// A `shell "<command>"` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAction {
    /// Source span of the `shell` keyword.
    pub keyword_span: Span,
    /// The literal command string.
    pub command: String,
    /// Full span of the action statement.
    pub span: Span,
}

/// Boolean guard expression as captured by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawGuard {
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
    /// semantic layer can validate `% 0` and `previous_outcome %` in one
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
        op: RawCmpOp,
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
    /// field name verbatim — only `previous_outcome` is meaningful here,
    /// but enforcing that is a *semantic* concern so the differential
    /// pest oracle (which only sees the syntactic shape) and the
    /// hand-written parser agree on accept/reject. The semantic layer
    /// rejects any other field with a "string RHS only valid for
    /// `previous_outcome`" diagnostic, then validates the value against
    /// the closed `"none" | "success" | "errored"` set.
    IterationOutcomeEq {
        /// LHS field name as written (e.g. `"previous_outcome"`,
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
    /// as [`Self::IterationOutcomeEq`].
    IterationOutcomeNeq {
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
    And(Box<RawGuard>, Box<RawGuard>, Span),
    /// Logical disjunction.
    Or(Box<RawGuard>, Box<RawGuard>, Span),
}

/// Comparison operator captured for `iteration.*` numeric predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawCmpOp {
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

impl RawGuard {
    /// Return the source span associated with this guard.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            RawGuard::MetadataEq { span, .. }
            | RawGuard::MetadataNeq { span, .. }
            | RawGuard::IterationCmp { span, .. }
            | RawGuard::IterationOutcomeEq { span, .. }
            | RawGuard::IterationOutcomeNeq { span, .. }
            | RawGuard::And(_, _, span)
            | RawGuard::Or(_, _, span) => span.clone(),
        }
    }
}
