//! Expressions shared by conditional language surfaces.

/// A dynamically resolved expression.
///
/// Paths are deliberately root-agnostic. The surface that owns an expression
/// decides which roots are available when it validates and evaluates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A JSON-shaped scalar literal.
    Literal(ExprLiteral),
    /// A root plus zero or more object/array traversal segments.
    Path {
        /// Root name, such as `metadata`, `iteration`, or `agent`.
        root: String,
        /// Traversal after the root.
        segments: Vec<PathSegment>,
    },
    /// A binary operation.
    Binary {
        /// Left operand.
        lhs: Box<Expr>,
        /// Operator.
        op: BinaryOp,
        /// Right operand.
        rhs: Box<Expr>,
    },
}

/// Scalar literal accepted in an [`Expr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprLiteral {
    /// String literal.
    String(String),
    /// Integer literal.
    Integer(i64),
    /// Boolean literal.
    Bool(bool),
    /// Null literal.
    Null,
}

/// One traversal segment in an expression path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// Object-field lookup (`.name`).
    Field(String),
    /// Array-index lookup (`[0]`).
    Index(usize),
}

/// Binary operators supported by the common expression language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
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
