//! [`PromptGuard`] — boolean expression used by
//! [`PromptSelector`](super::PromptSelector) to pick which
//! [`PromptTemplate`](super::PromptTemplate) to render for a given
//! [`Signal`].

use serde::{Deserialize, Serialize};

use crate::runner::iteration::{IterationContext, PreviousResult};
use crate::signal::Signal;
use crate::signal::metadata::MetadataValue;

/// Boolean expression used by [`PromptSelector`](super::PromptSelector) to
/// pick which [`PromptTemplate`](super::PromptTemplate) to render for a
/// given [`Signal`].
///
/// The shape mirrors the language's `PromptGuard` AST type but is kept
/// private to `iter_core` so the runtime does not depend on the language
/// crate. The operator (`iter_cli`) translates the AST into this form
/// before handing it to the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptGuard {
    /// `metadata.<key> == "<value>"`. Evaluates to `true` when the signal
    /// metadata has `key` bound to a stringified form equal to `value`.
    MetadataEq {
        /// Metadata key being compared.
        key: String,
        /// Literal string the key is compared against.
        value: String,
    },
    /// `metadata.<key> != "<value>"`. Evaluates to `true` when the key is
    /// either missing or its stringified form differs from `value`.
    MetadataNeq {
        /// Metadata key being compared.
        key: String,
        /// Literal string the key is compared against.
        value: String,
    },
    /// `iteration.<field> [% N] <op> <integer>`. Evaluates to a boolean by
    /// reading the numeric field of the current [`IterationContext`],
    /// optionally reducing it modulo `N`, and applying [`CmpOp`] against
    /// `rhs`. A missing `previous_exit_code` makes every comparison
    /// evaluate to `false` (intuitive "no previous iteration, nothing to
    /// compare against" semantics).
    IterationCmp {
        /// Numeric iteration field on the LHS.
        field: IterationField,
        /// Optional `% N` reduction applied to the LHS before comparison.
        modulus: Option<u32>,
        /// Comparison operator.
        op: CmpOp,
        /// Right-hand-side integer literal.
        rhs: i64,
    },
    /// `iteration.previous_result == "<value>"`. The accepted literals
    /// are exactly `"none" | "success" | "errored"` — anything else is
    /// rejected at semantic time so this variant only ever sees a valid
    /// result string.
    IterationResultEq {
        /// Result literal being compared against.
        value: String,
    },
    /// `iteration.previous_result != "<value>"`.
    IterationResultNeq {
        /// Result literal being compared against.
        value: String,
    },
    /// Logical conjunction. Evaluates `lhs && rhs`, short-circuiting on
    /// the first `false`.
    And(Box<PromptGuard>, Box<PromptGuard>),
    /// Logical disjunction. Evaluates `lhs || rhs`, short-circuiting on
    /// the first `true`.
    Or(Box<PromptGuard>, Box<PromptGuard>),
}

/// Numeric `iteration.*` field referenced by
/// [`PromptGuard::IterationCmp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationField {
    /// `iteration.count` — 1-indexed iteration number.
    Count,
    /// `iteration.previous_exit_code` — exit code captured on the
    /// previous iteration (`None` on the first iteration).
    PreviousExitCode,
    /// `iteration.consecutive_failures` — running failure-streak counter.
    ConsecutiveFailures,
    /// `iteration.consecutive_successes` — running success-streak
    /// counter.
    ConsecutiveSuccesses,
}

/// Comparison operator carried by [`PromptGuard::IterationCmp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl PromptGuard {
    /// Evaluate the guard against `signal` and `iteration`.
    ///
    /// Semantics:
    ///
    /// * Both `MetadataEq` and `MetadataNeq` are `false` when the key is
    ///   missing — an absent key is undecidable, so neither equality nor
    ///   inequality can be honestly answered.
    /// * `IterationCmp` reads the numeric `iteration.<field>` value,
    ///   optionally reduces it modulo `N`, and applies [`CmpOp`] against
    ///   `rhs`. A missing `previous_exit_code` makes every comparison —
    ///   including `!=` — evaluate to `false`. The "no previous iteration"
    ///   state is *not* represented as a number, so neither equality nor
    ///   inequality can be honestly answered; refusing to match is the
    ///   only consistent choice.
    /// * `IterationResultEq` / `IterationResultNeq` compare the
    ///   `previous_result` string against `value`. The semantic layer
    ///   restricts `value` to `"none" | "success" | "errored"`, so we
    ///   compare on the canonical string projection.
    #[must_use]
    pub fn matches(&self, signal: &Signal, iteration: &IterationContext) -> bool {
        match self {
            Self::MetadataEq { key, value } => {
                matches_metadata(signal, key).as_deref() == Some(value.as_str())
            }
            Self::MetadataNeq { key, value } => match matches_metadata(signal, key) {
                Some(ref v) => v.as_str() != value.as_str(),
                None => false,
            },
            Self::IterationCmp {
                field,
                modulus,
                op,
                rhs,
            } => match iteration_field_value(iteration, *field) {
                Some(lhs) => apply_cmp(reduce_modulus(lhs, *modulus), *op, *rhs),
                None => false,
            },
            Self::IterationResultEq { value } => {
                result_matches(iteration.previous_result, value.as_str())
            }
            Self::IterationResultNeq { value } => {
                !result_matches(iteration.previous_result, value.as_str())
            }
            Self::And(lhs, rhs) => lhs.matches(signal, iteration) && rhs.matches(signal, iteration),
            Self::Or(lhs, rhs) => lhs.matches(signal, iteration) || rhs.matches(signal, iteration),
        }
    }
}

/// Read the numeric value of `field` from the iteration context. Returns
/// `None` for `previous_exit_code` when no previous iteration has produced one
/// — at that point any numeric comparison is honestly undecidable, so
/// [`PromptGuard::matches`] surfaces the absence as a non-match rather
/// than picking an arbitrary sentinel like `-1`.
fn iteration_field_value(ctx: &IterationContext, field: IterationField) -> Option<i64> {
    match field {
        IterationField::Count => Some(i64::from(ctx.count)),
        IterationField::PreviousExitCode => ctx.previous_exit_code.map(i64::from),
        IterationField::ConsecutiveFailures => Some(i64::from(ctx.consecutive_failures)),
        IterationField::ConsecutiveSuccesses => Some(i64::from(ctx.consecutive_successes)),
    }
}

fn reduce_modulus(lhs: i64, modulus: Option<u32>) -> i64 {
    match modulus {
        Some(n) if n > 0 => lhs.rem_euclid(i64::from(n)),
        // semantic layer rejects modulus == 0; treat any leftover invalid
        // value as a no-op so we never panic in user code.
        _ => lhs,
    }
}

fn apply_cmp(lhs: i64, op: CmpOp, rhs: i64) -> bool {
    match op {
        CmpOp::Eq => lhs == rhs,
        CmpOp::Neq => lhs != rhs,
        CmpOp::Lt => lhs < rhs,
        CmpOp::Le => lhs <= rhs,
        CmpOp::Gt => lhs > rhs,
        CmpOp::Ge => lhs >= rhs,
    }
}

fn result_matches(previous: PreviousResult, expected: &str) -> bool {
    let actual = match previous {
        PreviousResult::None => "none",
        PreviousResult::Success => "success",
        PreviousResult::Errored => "errored",
    };
    actual == expected
}

/// Stringify a metadata value for guard comparison. Returns `None` when
/// the key is not present. `Null` values stringify to the empty string to
/// match the rendering rule used by the template renderer.
fn matches_metadata(signal: &Signal, key: &str) -> Option<String> {
    match signal.metadata().get_str(key)? {
        MetadataValue::Null => Some(String::new()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::test_helpers::{guard_kind_eq, signal_with, signal_with_kind};
    use crate::signal::metadata::Metadata;

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    #[test]
    fn guard_metadata_eq_matches_exact_value() {
        let signal = signal_with_kind("issue");
        assert!(guard_kind_eq("issue").matches(&signal, &iter_ctx()));
        assert!(!guard_kind_eq("ci_fix").matches(&signal, &iter_ctx()));
    }

    #[test]
    fn guard_metadata_eq_on_missing_key_is_false() {
        let signal = signal_with(Metadata::new());
        assert!(!guard_kind_eq("issue").matches(&signal, &iter_ctx()));
    }

    #[test]
    fn guard_metadata_neq_on_missing_key_is_false() {
        let signal = signal_with(Metadata::new());
        let guard = PromptGuard::MetadataNeq {
            key: "kind".into(),
            value: "issue".into(),
        };
        assert!(!guard.matches(&signal, &iter_ctx()));
    }

    #[test]
    fn guard_and_short_circuits() {
        let signal = signal_with_kind("issue");
        let guard = PromptGuard::And(
            Box::new(guard_kind_eq("issue")),
            Box::new(guard_kind_eq("ci_fix")),
        );
        assert!(!guard.matches(&signal, &iter_ctx()));

        let guard = PromptGuard::And(
            Box::new(guard_kind_eq("issue")),
            Box::new(guard_kind_eq("issue")),
        );
        assert!(guard.matches(&signal, &iter_ctx()));
    }

    #[test]
    fn guard_or_short_circuits() {
        let signal = signal_with_kind("issue");
        let guard = PromptGuard::Or(
            Box::new(guard_kind_eq("ci_fix")),
            Box::new(guard_kind_eq("issue")),
        );
        assert!(guard.matches(&signal, &iter_ctx()));

        let guard = PromptGuard::Or(
            Box::new(guard_kind_eq("ci_fix")),
            Box::new(guard_kind_eq("review_response")),
        );
        assert!(!guard.matches(&signal, &iter_ctx()));
    }
}
