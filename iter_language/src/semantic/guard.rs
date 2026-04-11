//! Lowering of [`RawGuard`] CST nodes into [`PromptGuard`] AST nodes.
//!
//! This is also where the iteration-guard well-formedness rules live:
//!
//! * `iteration.<field>` numeric comparisons accept only the four numeric
//!   fields (`count`, `previous_exit_code`, `consecutive_failures`,
//!   `consecutive_successes`). Any other field — including
//!   `previous_outcome` — is rejected with a tailored hint.
//! * `iteration.<field> % N <op> rhs` is rejected when `N == 0`.
//! * `iteration.previous_outcome ==/!= "..."` accepts only the literal
//!   outcomes `"none" | "success" | "errored"`.
//!
//! Errors collected here surface through the analyzer's diagnostic vector,
//! which means [`crate::parse`] returns `Err` for any non-well-formed
//! guard while the AST itself is still constructed (with sensible
//! placeholder values) so downstream lowering can keep collecting errors
//! rather than aborting on the first one.

use crate::ast::{CmpOp, IterationField, PromptGuard};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawCmpOp, RawGuard};

const ITERATION_NUMERIC_FIELDS_HINT: &str = "valid numeric `iteration.*` fields: `count`, `previous_exit_code`, `consecutive_failures`, `consecutive_successes`. Use `iteration.previous_outcome ==/!= \"none|success|errored\"` for the outcome string.";

const ITERATION_OUTCOME_VALUES: &[&str] = &["none", "success", "errored"];

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
                "previous_outcome" => {
                    errors.push(
                        Diagnostic::error(
                            field_span,
                            "`iteration.previous_outcome` is a string, not a number — numeric comparison is not supported",
                        )
                        .with_hint(
                            "use `iteration.previous_outcome == \"success\"` (or `!= \"...\"`); only `==`/`!=` against `\"none\" | \"success\" | \"errored\"` is accepted",
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
        RawGuard::IterationOutcomeEq {
            field,
            field_span,
            value,
            value_span,
            ..
        } => {
            check_outcome_string_rhs(&field, field_span, &value, value_span, errors);
            PromptGuard::IterationOutcomeEq { value }
        }
        RawGuard::IterationOutcomeNeq {
            field,
            field_span,
            value,
            value_span,
            ..
        } => {
            check_outcome_string_rhs(&field, field_span, &value, value_span, errors);
            PromptGuard::IterationOutcomeNeq { value }
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
/// The field check fires whenever the LHS isn't `previous_outcome`. The
/// value check is *gated* on the field being `previous_outcome`: if the
/// field is wrong, the literal "is `ten` a valid outcome?" question is
/// moot — the user's real mistake is the field name, and surfacing a
/// second "unknown outcome" diagnostic in addition is confusing noise.
/// We only check the value once we know the LHS is the outcome field.
fn check_outcome_string_rhs(
    field: &str,
    field_span: crate::ast::Span,
    value: &str,
    value_span: crate::ast::Span,
    errors: &mut Vec<Diagnostic>,
) {
    if field != "previous_outcome" {
        errors.push(
            Diagnostic::error(
                field_span,
                format!(
                    "string right-hand side is only valid for `iteration.previous_outcome`, but the left-hand side is `iteration.{field}`"
                ),
            )
            .with_hint(
                "numeric `iteration.*` fields require an integer RHS; only `previous_outcome` accepts a string",
            ),
        );
        return;
    }
    if !ITERATION_OUTCOME_VALUES.contains(&value) {
        errors.push(
            Diagnostic::error(
                value_span,
                format!(
                    "unknown iteration outcome `{value}` — accepted values are `none`, `success`, `errored`"
                ),
            )
            .with_hint(
                "`previous_outcome` is `\"none\"` on the first iteration, `\"success\"` after a clean turn, and `\"errored\"` after a stage error or non-zero agent exit",
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
