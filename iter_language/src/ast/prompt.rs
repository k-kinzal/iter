//! `prompt` declaration AST and shared priority keywords.

/// A `prompt [when <expr>] "<body>"` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDecl {
    /// Optional `when` guard expression.
    pub guard: Option<PromptGuard>,
    /// Raw template body. Placeholders such as `{{metadata.foo}}` are NOT
    /// resolved at parse time — that is the runner's responsibility.
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
    pub guard: PromptGuard,
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

/// Boolean expression accepted by `prompt when ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptGuard {
    /// `metadata.<key> == "<value>"`.
    MetadataEq {
        /// Metadata key being compared.
        key: String,
        /// Literal value compared against.
        value: String,
    },
    /// `metadata.<key> != "<value>"`.
    MetadataNeq {
        /// Metadata key being compared.
        key: String,
        /// Literal value compared against.
        value: String,
    },
    /// `iteration.<field> [% N] <op> <integer>` numeric comparison. The
    /// optional modulus reduces the LHS modulo `N` before applying the
    /// operator. Only the numeric fields of the iteration root reach
    /// this variant — `previous_outcome` is rejected at semantic time
    /// and surfaces as [`Self::IterationOutcomeEq`] /
    /// [`Self::IterationOutcomeNeq`] instead.
    IterationCmp {
        /// Numeric field of the iteration root being compared.
        field: IterationField,
        /// Optional modulus applied to the LHS before comparison. When
        /// `Some(n)`, the predicate evaluates `(lhs % n) <op> rhs`. The
        /// semantic layer rejects `n == 0`.
        modulus: Option<u32>,
        /// Comparison operator.
        op: CmpOp,
        /// Right-hand-side integer literal.
        rhs: i64,
    },
    /// `iteration.previous_outcome == "<value>"`. The semantic layer
    /// permits the literal `"none" | "success" | "errored"` only.
    IterationOutcomeEq {
        /// Literal outcome string compared against.
        value: String,
    },
    /// `iteration.previous_outcome != "<value>"`.
    IterationOutcomeNeq {
        /// Literal outcome string compared against.
        value: String,
    },
    /// Logical conjunction.
    And(Box<PromptGuard>, Box<PromptGuard>),
    /// Logical disjunction.
    Or(Box<PromptGuard>, Box<PromptGuard>),
}

/// Numeric `iteration.*` field referenced by an
/// [`PromptGuard::IterationCmp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterationField {
    /// `iteration.count` — 1-indexed iteration number.
    Count,
    /// `iteration.previous_exit_code` — process exit code captured on
    /// the previous iteration. A missing previous exit code makes every
    /// numeric comparison evaluate to `false`.
    PreviousExitCode,
    /// `iteration.consecutive_failures` — running failure-streak
    /// counter.
    ConsecutiveFailures,
    /// `iteration.consecutive_successes` — running success-streak
    /// counter.
    ConsecutiveSuccesses,
}

impl IterationField {
    /// Source-form name a guard uses to refer to this field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::PreviousExitCode => "previous_exit_code",
            Self::ConsecutiveFailures => "consecutive_failures",
            Self::ConsecutiveSuccesses => "consecutive_successes",
        }
    }
}

/// Comparison operator carried by [`PromptGuard::IterationCmp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
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

impl CmpOp {
    /// Source-form spelling of the operator.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Neq => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
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
