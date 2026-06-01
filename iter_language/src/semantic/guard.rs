//! Lowering of [`RawGuard`] CST nodes into [`PromptGuard`] AST nodes.
//!
//! This is also where the iteration-guard well-formedness rules live:
//!
//! * `iteration.<field>` numeric comparisons accept only the four numeric
//!   fields (`count`, `previous_exit_code`, `consecutive_failures`,
//!   `consecutive_successes`). Any other field — including
//!   `previous_result` — is rejected with a tailored hint.
//! * `iteration.<field> % N <op> rhs` is rejected when `N == 0`.
//! * `iteration.previous_result ==/!= "..."` accepts only the literal
//!   values `"none" | "success" | "errored"`.
//!
//! Errors collected here surface through the analyzer's diagnostic vector,
//! which means [`crate::parse`] returns `Err` for any non-well-formed
//! guard while the AST itself is still constructed (with sensible
//! placeholder values) so downstream lowering can keep collecting errors
//! rather than aborting on the first one.

use crate::ast::{CmpOp, IterationField, PromptGuard};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawCmpOp, RawGuard};

const ITERATION_NUMERIC_FIELDS_HINT: &str = "valid numeric `iteration.*` fields: `count`, `previous_exit_code`, `consecutive_failures`, `consecutive_successes`. Use `iteration.previous_result ==/!= \"none|success|errored\"` for the result string.";

const ITERATION_RESULT_VALUES: &[&str] = &["none", "success", "errored"];

pub(super) fn lower_guard_pure(guard: RawGuard, errors: &mut Vec<Diagnostic>) -> PromptGuard {
    match guard {
        RawGuard::MetadataEq { key, value, .. } => PromptGuard::MetadataEq { key, value },
        RawGuard::MetadataNeq { key, value, .. } => PromptGuard::MetadataNeq { key, value },
        RawGuard::IterationCmp {
            field,
            field_span,
            modulus,
            modulus_span,
            op,
            rhs,
            ..
        } => {
            let lowered_field = match field.as_str() {
                "count" => IterationField::Count,
                "previous_exit_code" => IterationField::PreviousExitCode,
                "consecutive_failures" => IterationField::ConsecutiveFailures,
                "consecutive_successes" => IterationField::ConsecutiveSuccesses,
                "previous_result" => {
                    errors.push(
                        Diagnostic::error(
                            field_span,
                            "`iteration.previous_result` is a string, not a number — numeric comparison is not supported",
                        )
                        .with_hint(
                            "use `iteration.previous_result == \"success\"` (or `!= \"...\"`); only `==`/`!=` against `\"none\" | \"success\" | \"errored\"` is accepted",
                        ),
                    );
                    // Fall back to a numeric field so the AST shape is
                    // representable; the error severity prevents the
                    // caller from returning a `Root` anyway.
                    IterationField::Count
                }
                other => {
                    errors.push(
                        Diagnostic::error(field_span, format!("unknown iteration field `{other}`"))
                            .with_hint(ITERATION_NUMERIC_FIELDS_HINT),
                    );
                    IterationField::Count
                }
            };

            let lowered_modulus = match modulus {
                Some(0) => {
                    if let Some(span) = modulus_span {
                        errors.push(Diagnostic::error(
                            span,
                            "`% 0` is not a valid iteration-guard modulus",
                        ));
                    }
                    None
                }
                Some(n) if n < 0 => {
                    if let Some(span) = modulus_span {
                        errors.push(Diagnostic::error(
                            span,
                            format!("iteration-guard modulus must be a positive integer, got {n}"),
                        ));
                    }
                    None
                }
                Some(n) => Some(u32::try_from(n).unwrap_or(u32::MAX)),
                None => None,
            };

            PromptGuard::IterationCmp {
                field: lowered_field,
                modulus: lowered_modulus,
                op: lower_cmp_op(op),
                rhs,
            }
        }
        RawGuard::IterationResultEq {
            field,
            field_span,
            value,
            value_span,
            ..
        } => {
            check_result_string_rhs(&field, field_span, &value, value_span, errors);
            PromptGuard::IterationResultEq { value }
        }
        RawGuard::IterationResultNeq {
            field,
            field_span,
            value,
            value_span,
            ..
        } => {
            check_result_string_rhs(&field, field_span, &value, value_span, errors);
            PromptGuard::IterationResultNeq { value }
        }
        RawGuard::And(l, r, _) => PromptGuard::And(
            Box::new(lower_guard_pure(*l, errors)),
            Box::new(lower_guard_pure(*r, errors)),
        ),
        RawGuard::Or(l, r, _) => PromptGuard::Or(
            Box::new(lower_guard_pure(*l, errors)),
            Box::new(lower_guard_pure(*r, errors)),
        ),
    }
}

/// Validate the field/value pair behind `iteration.<field> ==/!= "..."`.
///
/// The field check fires whenever the LHS isn't `previous_result`. The
/// value check is *gated* on the field being `previous_result`: if the
/// field is wrong, the literal "is `ten` a valid result?" question is
/// moot — the user's real mistake is the field name, and surfacing a
/// second "unknown result" diagnostic in addition is confusing noise.
/// We only check the value once we know the LHS is the result field.
fn check_result_string_rhs(
    field: &str,
    field_span: crate::ast::Span,
    value: &str,
    value_span: crate::ast::Span,
    errors: &mut Vec<Diagnostic>,
) {
    if field != "previous_result" {
        errors.push(
            Diagnostic::error(
                field_span,
                format!(
                    "string right-hand side is only valid for `iteration.previous_result`, but the left-hand side is `iteration.{field}`"
                ),
            )
            .with_hint(
                "numeric `iteration.*` fields require an integer RHS; only `previous_result` accepts a string",
            ),
        );
        return;
    }
    if !ITERATION_RESULT_VALUES.contains(&value) {
        errors.push(
            Diagnostic::error(
                value_span,
                format!(
                    "unknown iteration result `{value}` — accepted values are `none`, `success`, `errored`"
                ),
            )
            .with_hint(
                "`previous_result` is `\"none\"` on the first iteration, `\"success\"` after a clean turn, and `\"errored\"` after a stage error or non-zero agent exit",
            ),
        );
    }
}

fn lower_cmp_op(op: RawCmpOp) -> CmpOp {
    match op {
        RawCmpOp::Eq => CmpOp::Eq,
        RawCmpOp::Neq => CmpOp::Neq,
        RawCmpOp::Lt => CmpOp::Lt,
        RawCmpOp::Le => CmpOp::Le,
        RawCmpOp::Gt => CmpOp::Gt,
        RawCmpOp::Ge => CmpOp::Ge,
    }
}
