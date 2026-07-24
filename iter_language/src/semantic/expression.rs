//! Lowering and contextual validation for common expressions.

use crate::ast::{BinaryOp, Expr, ExprLiteral, PathSegment};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBinaryOp, CstExpr, CstExprLiteral, CstPathSegment};

pub(super) fn lower_expr_pure(
    expression: CstExpr,
    errors: &mut Vec<Diagnostic>,
    allowed_roots: &[&str],
) -> Expr {
    let expression_type = infer_type(&expression, errors);
    if !matches!(expression_type, ExprType::Bool | ExprType::Unknown) {
        errors.push(Diagnostic::error(
            expression.span(),
            format!(
                "condition expression must evaluate to a boolean, found {}",
                type_name(expression_type)
            ),
        ));
    }
    lower_expr(expression, errors, allowed_roots)
}

fn lower_expr(expression: CstExpr, errors: &mut Vec<Diagnostic>, allowed_roots: &[&str]) -> Expr {
    match expression {
        CstExpr::Literal { value, .. } => Expr::Literal(lower_literal(value)),
        CstExpr::Path {
            root,
            segments,
            span,
        } => {
            validate_path(&root.name, &segments, span, allowed_roots, errors);
            Expr::Path {
                root: root.name,
                segments: segments.into_iter().map(lower_path_segment).collect(),
            }
        }
        CstExpr::Binary {
            lhs,
            op,
            op_span,
            rhs,
            ..
        } => {
            if matches!(op, CstBinaryOp::Mod)
                && matches!(
                    rhs.as_ref(),
                    CstExpr::Literal {
                        value: CstExprLiteral::Integer(0),
                        ..
                    }
                )
            {
                errors.push(Diagnostic::error(op_span, "`% 0` is not valid"));
            }
            Expr::Binary {
                lhs: Box::new(lower_expr(*lhs, errors, allowed_roots)),
                op: lower_binary_op(op),
                rhs: Box::new(lower_expr(*rhs, errors, allowed_roots)),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprType {
    String,
    Integer,
    Bool,
    Null,
    Object,
    Unknown,
}

fn infer_type(expression: &CstExpr, errors: &mut Vec<Diagnostic>) -> ExprType {
    match expression {
        CstExpr::Literal { value, .. } => match value {
            CstExprLiteral::String(_) => ExprType::String,
            CstExprLiteral::Integer(_) => ExprType::Integer,
            CstExprLiteral::Bool(_) => ExprType::Bool,
            CstExprLiteral::Null => ExprType::Null,
        },
        CstExpr::Path { root, segments, .. } => path_type(&root.name, segments),
        CstExpr::Binary {
            lhs,
            op,
            op_span,
            rhs,
            ..
        } => {
            validate_closed_string_value(lhs, *op, rhs, errors);
            validate_closed_string_value(rhs, *op, lhs, errors);
            let lhs_type = infer_type(lhs, errors);
            let rhs_type = infer_type(rhs, errors);
            match op {
                CstBinaryOp::Or | CstBinaryOp::And => {
                    require_type(lhs_type, ExprType::Bool, op_span, errors);
                    require_type(rhs_type, ExprType::Bool, op_span, errors);
                    ExprType::Bool
                }
                CstBinaryOp::Eq | CstBinaryOp::Neq => {
                    if !matches!(lhs_type, ExprType::Unknown | ExprType::Null)
                        && !matches!(rhs_type, ExprType::Unknown | ExprType::Null)
                        && lhs_type != rhs_type
                    {
                        errors.push(Diagnostic::error(
                            op_span.clone(),
                            "equality operands have incompatible types",
                        ));
                    }
                    ExprType::Bool
                }
                CstBinaryOp::Lt | CstBinaryOp::Le | CstBinaryOp::Gt | CstBinaryOp::Ge => {
                    if !matches!(lhs_type, ExprType::Unknown)
                        && !matches!(rhs_type, ExprType::Unknown)
                        && (lhs_type != rhs_type
                            || !matches!(lhs_type, ExprType::Integer | ExprType::String))
                    {
                        errors.push(Diagnostic::error(
                            op_span.clone(),
                            "ordering operands must both be integers or both be strings",
                        ));
                    }
                    ExprType::Bool
                }
                CstBinaryOp::Mod => {
                    require_type(lhs_type, ExprType::Integer, op_span, errors);
                    require_type(rhs_type, ExprType::Integer, op_span, errors);
                    ExprType::Integer
                }
            }
        }
    }
}

fn require_type(
    actual: ExprType,
    expected: ExprType,
    span: &crate::ast::Span,
    errors: &mut Vec<Diagnostic>,
) {
    if !matches!(actual, ExprType::Unknown) && actual != expected {
        errors.push(Diagnostic::error(
            span.clone(),
            format!(
                "expression operand must be {}, found {}",
                type_name(expected),
                type_name(actual)
            ),
        ));
    }
}

fn type_name(kind: ExprType) -> &'static str {
    match kind {
        ExprType::String => "a string",
        ExprType::Integer => "an integer",
        ExprType::Bool => "a boolean",
        ExprType::Null => "null",
        ExprType::Object => "an object",
        ExprType::Unknown => "a dynamic value",
    }
}

fn path_type(root: &str, segments: &[CstPathSegment]) -> ExprType {
    if segments.is_empty() {
        return ExprType::Object;
    }
    let first = segments.first().and_then(|segment| match segment {
        CstPathSegment::Field(field) => Some(field.name.as_str()),
        CstPathSegment::Index(_, _) => None,
    });
    match (root, first) {
        (
            "iteration",
            Some("count" | "previous_exit_code" | "consecutive_failures" | "consecutive_successes"),
        ) => ExprType::Integer,
        (
            "iteration",
            Some("previous_result" | "runner_started_at" | "started_at" | "previous_signal_id"),
        )
        | ("agent", Some("session_id"))
        | ("signal", Some("id" | "created_at"))
        | ("metadata", Some(_)) => ExprType::String,
        _ => ExprType::Unknown,
    }
}

fn validate_path(
    root: &str,
    segments: &[CstPathSegment],
    span: crate::ast::Span,
    allowed_roots: &[&str],
    errors: &mut Vec<Diagnostic>,
) {
    if !allowed_roots.contains(&root) {
        errors.push(
            Diagnostic::error(
                span.clone(),
                format!("expression root `{root}` is not available here"),
            )
            .with_hint(format!(
                "available roots: {}",
                allowed_roots
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        );
        return;
    }
    let first_field = segments.first().and_then(|segment| match segment {
        CstPathSegment::Field(field) => Some(field.name.as_str()),
        CstPathSegment::Index(_, _) => None,
    });
    match root {
        "iteration" => {
            const FIELDS: &[&str] = &[
                "count",
                "previous_exit_code",
                "previous_result",
                "consecutive_failures",
                "consecutive_successes",
                "runner_started_at",
                "started_at",
                "previous_signal_id",
            ];
            if let Some(field) = first_field {
                if !FIELDS.contains(&field) {
                    errors.push(Diagnostic::error(
                        span,
                        format!("unknown iteration field `{field}`"),
                    ));
                }
            }
        }
        "agent" => {
            if let Some(field) = first_field {
                if !matches!(field, "session_id" | "output") {
                    errors.push(Diagnostic::error(
                        span,
                        format!("unknown agent field `{field}`"),
                    ));
                }
            }
        }
        _ => {}
    }
}

fn validate_closed_string_value(
    path: &CstExpr,
    op: CstBinaryOp,
    value: &CstExpr,
    errors: &mut Vec<Diagnostic>,
) {
    if !matches!(op, CstBinaryOp::Eq | CstBinaryOp::Neq)
        || !is_direct_path(path, "iteration", "previous_result")
    {
        return;
    }
    let CstExpr::Literal {
        value: CstExprLiteral::String(value),
        span,
    } = value
    else {
        return;
    };
    if !matches!(value.as_str(), "none" | "success" | "errored") {
        errors.push(
            Diagnostic::error(
                span.clone(),
                format!("unknown iteration.previous_result value `{value}`"),
            )
            .with_hint("expected one of `none`, `success`, or `errored`"),
        );
    }
}

fn is_direct_path(expression: &CstExpr, root_name: &str, field_name: &str) -> bool {
    matches!(
        expression,
        CstExpr::Path {
            root,
            segments,
            ..
        } if root.name == root_name
            && matches!(
                segments.as_slice(),
                [CstPathSegment::Field(field)] if field.name == field_name
            )
    )
}

fn lower_literal(literal: CstExprLiteral) -> ExprLiteral {
    match literal {
        CstExprLiteral::String(value) => ExprLiteral::String(value),
        CstExprLiteral::Integer(value) => ExprLiteral::Integer(value),
        CstExprLiteral::Bool(value) => ExprLiteral::Bool(value),
        CstExprLiteral::Null => ExprLiteral::Null,
    }
}

fn lower_path_segment(segment: CstPathSegment) -> PathSegment {
    match segment {
        CstPathSegment::Field(field) => PathSegment::Field(field.name),
        CstPathSegment::Index(index, _) => PathSegment::Index(index),
    }
}

fn lower_binary_op(op: CstBinaryOp) -> BinaryOp {
    match op {
        CstBinaryOp::Or => BinaryOp::Or,
        CstBinaryOp::And => BinaryOp::And,
        CstBinaryOp::Eq => BinaryOp::Eq,
        CstBinaryOp::Neq => BinaryOp::Neq,
        CstBinaryOp::Lt => BinaryOp::Lt,
        CstBinaryOp::Le => BinaryOp::Le,
        CstBinaryOp::Gt => BinaryOp::Gt,
        CstBinaryOp::Ge => BinaryOp::Ge,
        CstBinaryOp::Mod => BinaryOp::Mod,
    }
}
