//! Runtime expression AST and evaluator.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A root-agnostic expression evaluated against a JSON-shaped context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// Scalar literal.
    Literal(ExprLiteral),
    /// Rooted object/array path.
    Path {
        /// Root name.
        root: String,
        /// Traversal segments.
        segments: Vec<PathSegment>,
    },
    /// Binary operation.
    Binary {
        /// Left operand.
        lhs: Box<Expr>,
        /// Operator.
        op: BinaryOp,
        /// Right operand.
        rhs: Box<Expr>,
    },
}

/// Scalar expression literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Path traversal segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathSegment {
    /// Object field.
    Field(String),
    /// Array index.
    Index(usize),
}

/// Binary expression operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Expression evaluation failure.
#[derive(Debug, thiserror::Error)]
pub enum ExprError {
    /// A boolean position received another value type.
    #[error("expression must evaluate to a boolean")]
    ExpectedBoolean,
    /// An operator received incompatible operand types.
    #[error("operator {operator} does not accept these operands")]
    InvalidOperands {
        /// Source spelling of the operator.
        operator: &'static str,
    },
    /// Integer remainder by zero.
    #[error("expression attempted remainder by zero")]
    ModuloByZero,
    /// A render context could not be represented as JSON.
    #[error("failed to construct expression context: {0}")]
    Context(#[from] serde_json::Error),
}

enum EvalValue {
    Defined(Value),
    Undefined,
}

impl Expr {
    /// Evaluate this expression as a boolean.
    ///
    /// A missing path is a non-match. In particular, both equality and
    /// inequality return `false` when either operand is missing.
    ///
    /// # Errors
    ///
    /// Returns [`ExprError`] for type-invalid operations or a non-boolean
    /// final value.
    pub fn evaluate_bool(&self, context: &Value) -> Result<bool, ExprError> {
        match self.evaluate(context)? {
            EvalValue::Defined(Value::Bool(value)) => Ok(value),
            EvalValue::Undefined => Ok(false),
            EvalValue::Defined(_) => Err(ExprError::ExpectedBoolean),
        }
    }

    fn evaluate(&self, context: &Value) -> Result<EvalValue, ExprError> {
        match self {
            Self::Literal(literal) => Ok(EvalValue::Defined(literal_value(literal))),
            Self::Path { root, segments } => Ok(resolve_path(context, root, segments)
                .cloned()
                .map_or(EvalValue::Undefined, EvalValue::Defined)),
            Self::Binary { lhs, op, rhs } => match op {
                BinaryOp::And => {
                    let left = lhs.evaluate_bool(context)?;
                    if !left {
                        return Ok(EvalValue::Defined(Value::Bool(false)));
                    }
                    Ok(EvalValue::Defined(Value::Bool(rhs.evaluate_bool(context)?)))
                }
                BinaryOp::Or => {
                    let left = lhs.evaluate_bool(context)?;
                    if left {
                        return Ok(EvalValue::Defined(Value::Bool(true)));
                    }
                    Ok(EvalValue::Defined(Value::Bool(rhs.evaluate_bool(context)?)))
                }
                BinaryOp::Eq | BinaryOp::Neq => {
                    let left = lhs.evaluate(context)?;
                    let right = rhs.evaluate(context)?;
                    let (EvalValue::Defined(left), EvalValue::Defined(right)) = (left, right)
                    else {
                        return Ok(EvalValue::Defined(Value::Bool(false)));
                    };
                    // A nullable value compared with a non-null value is an
                    // unavailable operand, not proof of inequality. This
                    // preserves the pre-expression behavior of optional
                    // iteration fields on the first iteration while keeping
                    // explicit `value == null` checks meaningful.
                    if matches!(
                        (&left, &right),
                        (Value::Null, value) | (value, Value::Null)
                            if !value.is_null()
                    ) {
                        return Ok(EvalValue::Defined(Value::Bool(false)));
                    }
                    let equal = values_equal(&left, &right);
                    Ok(EvalValue::Defined(Value::Bool(
                        if matches!(op, BinaryOp::Eq) {
                            equal
                        } else {
                            !equal
                        },
                    )))
                }
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                    compare_order(lhs.evaluate(context)?, *op, rhs.evaluate(context)?)
                }
                BinaryOp::Mod => modulo(lhs.evaluate(context)?, rhs.evaluate(context)?),
            },
        }
    }
}

fn literal_value(literal: &ExprLiteral) -> Value {
    match literal {
        ExprLiteral::String(value) => Value::String(value.clone()),
        ExprLiteral::Integer(value) => Value::Number((*value).into()),
        ExprLiteral::Bool(value) => Value::Bool(*value),
        ExprLiteral::Null => Value::Null,
    }
}

fn resolve_path<'a>(context: &'a Value, root: &str, segments: &[PathSegment]) -> Option<&'a Value> {
    let mut current = context.get(root)?;
    for segment in segments {
        current = match segment {
            PathSegment::Field(field) => current.get(field)?,
            PathSegment::Index(index) => current.get(*index)?,
        };
    }
    Some(current)
}

fn compare_order(lhs: EvalValue, op: BinaryOp, rhs: EvalValue) -> Result<EvalValue, ExprError> {
    let (EvalValue::Defined(lhs), EvalValue::Defined(rhs)) = (lhs, rhs) else {
        return Ok(EvalValue::Defined(Value::Bool(false)));
    };
    if lhs.is_null() || rhs.is_null() {
        return Ok(EvalValue::Defined(Value::Bool(false)));
    }
    let ordering = match (&lhs, &rhs) {
        (Value::Number(left), Value::Number(right)) => compare_numbers(left, right),
        (Value::String(left), Value::String(right)) => left.cmp(right),
        _ => return Err(invalid_operands(op)),
    };
    let matched = match op {
        BinaryOp::Lt => ordering.is_lt(),
        BinaryOp::Le => ordering.is_le(),
        BinaryOp::Gt => ordering.is_gt(),
        BinaryOp::Ge => ordering.is_ge(),
        BinaryOp::Or | BinaryOp::And | BinaryOp::Eq | BinaryOp::Neq | BinaryOp::Mod => {
            return Err(invalid_operands(op));
        }
    };
    Ok(EvalValue::Defined(Value::Bool(matched)))
}

fn modulo(lhs: EvalValue, rhs: EvalValue) -> Result<EvalValue, ExprError> {
    if matches!(
        (&lhs, &rhs),
        (EvalValue::Undefined | EvalValue::Defined(Value::Null), _)
            | (_, EvalValue::Undefined | EvalValue::Defined(Value::Null))
    ) {
        return Ok(EvalValue::Undefined);
    }
    let (EvalValue::Defined(Value::Number(lhs)), EvalValue::Defined(Value::Number(rhs))) =
        (lhs, rhs)
    else {
        return Err(invalid_operands(BinaryOp::Mod));
    };
    let (Some(lhs), Some(rhs)) = (lhs.as_i64(), rhs.as_i64()) else {
        return Err(invalid_operands(BinaryOp::Mod));
    };
    if rhs == 0 {
        return Err(ExprError::ModuloByZero);
    }
    Ok(EvalValue::Defined(Value::Number(
        lhs.rem_euclid(rhs).into(),
    )))
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => compare_numbers(left, right).is_eq(),
        _ => left == right,
    }
}

fn compare_numbers(left: &serde_json::Number, right: &serde_json::Number) -> std::cmp::Ordering {
    match (integer_value(left), integer_value(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left
            .as_f64()
            .expect("serde_json numbers are finite JSON numbers")
            .partial_cmp(
                &right
                    .as_f64()
                    .expect("serde_json numbers are finite JSON numbers"),
            )
            .expect("serde_json numbers cannot be NaN"),
    }
}

fn integer_value(number: &serde_json::Number) -> Option<i128> {
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}

fn invalid_operands(op: BinaryOp) -> ExprError {
    ExprError::InvalidOperands {
        operator: match op {
            BinaryOp::Or => "||",
            BinaryOp::And => "&&",
            BinaryOp::Eq => "==",
            BinaryOp::Neq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Mod => "%",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn path(root: &str, fields: &[&str]) -> Expr {
        Expr::Path {
            root: root.to_owned(),
            segments: fields
                .iter()
                .map(|field| PathSegment::Field((*field).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn resolves_nested_json_fields() {
        let expression = Expr::Binary {
            lhs: Box::new(path("agent", &["output", "decision"])),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::String("pass".into()))),
        };
        assert!(
            expression
                .evaluate_bool(&json!({"agent":{"output":{"decision":"pass"}}}))
                .expect("evaluate")
        );
    }

    #[test]
    fn missing_path_is_not_equal_or_unequal() {
        for op in [BinaryOp::Eq, BinaryOp::Neq] {
            let expression = Expr::Binary {
                lhs: Box::new(path("agent", &["output"])),
                op,
                rhs: Box::new(Expr::Literal(ExprLiteral::String("pass".into()))),
            };
            assert!(!expression.evaluate_bool(&json!({})).expect("evaluate"));
        }
    }

    #[test]
    fn null_optional_value_is_not_equal_or_unequal_to_a_concrete_value() {
        for op in [BinaryOp::Eq, BinaryOp::Neq] {
            let expression = Expr::Binary {
                lhs: Box::new(path("iteration", &["previous_exit_code"])),
                op,
                rhs: Box::new(Expr::Literal(ExprLiteral::Integer(0))),
            };
            assert!(
                !expression
                    .evaluate_bool(&json!({"iteration":{"previous_exit_code":null}}))
                    .expect("evaluate")
            );
        }
    }

    #[test]
    fn null_optional_value_is_false_for_ordering_and_modulo() {
        let previous_exit_code = path("iteration", &["previous_exit_code"]);
        let ordering = Expr::Binary {
            lhs: Box::new(previous_exit_code.clone()),
            op: BinaryOp::Gt,
            rhs: Box::new(Expr::Literal(ExprLiteral::Integer(0))),
        };
        let modulo = Expr::Binary {
            lhs: Box::new(Expr::Binary {
                lhs: Box::new(previous_exit_code),
                op: BinaryOp::Mod,
                rhs: Box::new(Expr::Literal(ExprLiteral::Integer(2))),
            }),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::Integer(0))),
        };
        let context = json!({"iteration":{"previous_exit_code":null}});
        assert!(!ordering.evaluate_bool(&context).expect("ordering"));
        assert!(!modulo.evaluate_bool(&context).expect("modulo"));
    }

    #[test]
    fn explicit_null_equality_remains_available() {
        let expression = Expr::Binary {
            lhs: Box::new(path("agent", &["output", "reason"])),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::Null)),
        };
        assert!(
            expression
                .evaluate_bool(&json!({"agent":{"output":{"reason":null}}}))
                .expect("evaluate")
        );
    }

    #[test]
    fn compares_integer_and_fractional_json_numbers() {
        let greater = Expr::Binary {
            lhs: Box::new(path("agent", &["output", "score"])),
            op: BinaryOp::Gt,
            rhs: Box::new(Expr::Literal(ExprLiteral::Integer(0))),
        };
        let equal = Expr::Binary {
            lhs: Box::new(path("agent", &["output", "count"])),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::Integer(1))),
        };
        let context = json!({"agent":{"output":{"score":0.87,"count":1.0}}});
        assert!(greater.evaluate_bool(&context).expect("greater"));
        assert!(equal.evaluate_bool(&context).expect("equal"));
    }
}
