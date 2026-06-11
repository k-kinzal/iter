//! `{{...}}` template-placeholder validation, scoped by template position.
//!
//! Every template string sits in a *position* (R8). The position fixes
//! which `{{...}}` roots are legal there: a prompt body reads `signal`,
//! `metadata`, `iteration`, and `today`; a webhook subscription adds
//! `event`; a queue dead-letter template sees only `error`. A reference that
//! is not legal for its position is a hard **error** — validation is
//! authoritative here, not advisory, and nothing downstream re-decides it.
//!
//! `{{arg.*}}` is a separate axis. It is accepted syntactically in every
//! position, recorded as it is seen, and cross-checked against the file's
//! declared `arg`s once the whole file is lowered (the operator substitutes
//! it at plan time, so it never reaches a runtime template). The arg-name
//! grammar accepted here is exactly the one [`super::is_valid_arg_name`]
//! enforces on `arg` declarations — a name that can be declared and a name
//! that can be referenced are the same grammar.

use super::{Analyzer, is_valid_arg_name};
use crate::ast::Span;
use crate::diagnostic::Diagnostic;

/// Where a template string appears. The position determines which `{{...}}`
/// roots are legal (R8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplatePosition {
    /// `prompt = "..."` bodies, named prompts, and prompt match arms.
    Prompt,
    /// `on <event> { shell "..." }` action commands.
    ShellAction,
    /// A webhook subscription's `when` guard and per-subscription metadata.
    WebhookSubscription,
    /// A trigger's base `metadata { ... }` (stamped before any event, so no
    /// `event` root is available).
    TriggerBaseMetadata,
    /// A queue dead-letter `reason_template` / `description_template`,
    /// rendered by core at failure time against the failure only.
    DeadLetterReason,
}

impl TemplatePosition {
    /// The dotted runtime roots legal in this position, excluding the
    /// universal `arg.*` plan-time axis and the bare `today` root.
    fn dotted_roots(self) -> &'static [&'static str] {
        match self {
            TemplatePosition::Prompt
            | TemplatePosition::ShellAction
            | TemplatePosition::TriggerBaseMetadata => &["signal", "metadata", "iteration"],
            TemplatePosition::WebhookSubscription => &["signal", "metadata", "iteration", "event"],
            TemplatePosition::DeadLetterReason => &["error"],
        }
    }

    /// Whether the bare `{{today}}` root is legal here.
    fn allows_today(self) -> bool {
        !matches!(self, TemplatePosition::DeadLetterReason)
    }

    /// Human-facing list of the forms legal in this position.
    fn hint(self) -> String {
        let mut forms: Vec<String> = self
            .dotted_roots()
            .iter()
            .map(|r| format!("`{r}.<field>`"))
            .collect();
        if self.allows_today() {
            forms.push("`today`".to_string());
        }
        forms.push("`arg.<name>`".to_string());
        format!("valid forms here: {}", forms.join(", "))
    }
}

impl Analyzer {
    /// Walk `body` for `{{...}}` placeholders and check each reference
    /// against `position`. Unterminated/empty placeholders and roots that
    /// are illegal for the position are errors; `{{arg.<name>}}` references
    /// are recorded for the later declared-name cross-check
    /// ([`Analyzer::finish_arg_refs`]).
    pub(super) fn validate_template(
        &mut self,
        body: &str,
        span: &Span,
        position: TemplatePosition,
    ) {
        let bytes = body.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'{' && bytes[i + 1] == b'{' {
                let start = i;
                i += 2;
                let inner_start = i;
                while i + 1 < bytes.len() && !(bytes[i] == b'}' && bytes[i + 1] == b'}') {
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    let abs = span.start + start;
                    self.errors.push(Diagnostic::error(
                        abs..span.end.min(span.start + bytes.len()),
                        "unterminated `{{...}}` template placeholder",
                    ));
                    return;
                }
                let abs_start = span.start + start;
                let abs_end = span.start + i + 2;
                let inner = body[inner_start..i].trim().to_string();
                self.check_template_ref(&inner, abs_start..abs_end, position);
                i += 2;
            } else {
                i += 1;
            }
        }
    }

    /// Check a single `{{...}}` inner reference against `position`.
    fn check_template_ref(&mut self, inner: &str, span: Span, position: TemplatePosition) {
        if inner.is_empty() {
            self.errors
                .push(Diagnostic::error(span, "empty `{{}}` template placeholder"));
            return;
        }

        // The `arg.*` axis is universal and cross-checked after lowering.
        if inner == "arg" {
            self.errors.push(
                Diagnostic::error(span, "invalid template reference `arg`")
                    .with_hint("argument references are written `arg.<name>`"),
            );
            return;
        }
        if let Some(name) = inner.strip_prefix("arg.") {
            if name.is_empty() || name.contains('.') || !is_valid_arg_name(name) {
                self.errors.push(
                    Diagnostic::error(span, format!("invalid argument reference `{inner}`"))
                        .with_hint(
                            "`arg.<name>` references a declared `arg`; names start with a letter or `_` and contain only ASCII alphanumerics and `_` (no `-`)",
                        ),
                );
            } else {
                self.arg_refs.push((name.to_string(), span));
            }
            return;
        }

        // The bare `today` root carries no field.
        if inner == "today" {
            if !position.allows_today() {
                self.errors.push(
                    Diagnostic::error(span, "`today` is not available here")
                        .with_hint(position.hint()),
                );
            }
            return;
        }

        let mut parts = inner.split('.');
        let head = parts.next().unwrap_or("");
        let tail: Vec<&str> = parts.collect();
        if head == "today" {
            self.errors.push(Diagnostic::error(
                span,
                "`today` does not take a field; use `{{today}}` for the current date",
            ));
            return;
        }
        if !position.dotted_roots().contains(&head) {
            self.errors.push(
                Diagnostic::error(span, format!("invalid template reference `{inner}`"))
                    .with_hint(position.hint()),
            );
            return;
        }
        if tail.is_empty() {
            self.errors.push(
                Diagnostic::error(
                    span,
                    format!("`{head}` requires a field, e.g. `{head}.<field>`"),
                )
                .with_hint(position.hint()),
            );
            return;
        }
        for seg in &tail {
            if seg.is_empty()
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                self.errors.push(
                    Diagnostic::error(span, format!("invalid template reference `{inner}`"))
                        .with_hint(position.hint()),
                );
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `true` when `body` produces no diagnostics in `position`.
    fn accepts(body: &str, position: TemplatePosition) -> bool {
        let mut a = Analyzer::default();
        a.validate_template(body, &(0..body.len()), position);
        a.errors.is_empty()
    }

    #[test]
    fn prompt_position_roots() {
        assert!(accepts("{{signal.id}}", TemplatePosition::Prompt));
        assert!(accepts(
            "{{metadata.foo}} {{iteration.count}} {{today}}",
            TemplatePosition::Prompt
        ));
        // `event` is only legal in a webhook subscription.
        assert!(!accepts("{{event.action}}", TemplatePosition::Prompt));
        // `error` is only legal in a dead-letter template.
        assert!(!accepts("{{error.kind}}", TemplatePosition::Prompt));
    }

    #[test]
    fn shell_action_matches_prompt_minus_event() {
        assert!(accepts("{{signal.id}}", TemplatePosition::ShellAction));
        assert!(!accepts("{{event.action}}", TemplatePosition::ShellAction));
    }

    #[test]
    fn webhook_subscription_accepts_event() {
        assert!(accepts(
            "{{event.repository.full_name}}",
            TemplatePosition::WebhookSubscription
        ));
        assert!(accepts("{{today}}", TemplatePosition::WebhookSubscription));
    }

    #[test]
    fn trigger_base_metadata_rejects_event() {
        assert!(!accepts(
            "{{event.action}}",
            TemplatePosition::TriggerBaseMetadata
        ));
        assert!(accepts("{{today}}", TemplatePosition::TriggerBaseMetadata));
    }

    #[test]
    fn dead_letter_accepts_error_only() {
        assert!(accepts(
            "{{error.kind}}",
            TemplatePosition::DeadLetterReason
        ));
        assert!(!accepts(
            "{{signal.id}}",
            TemplatePosition::DeadLetterReason
        ));
        assert!(!accepts("{{today}}", TemplatePosition::DeadLetterReason));
        assert!(!accepts("{{error}}", TemplatePosition::DeadLetterReason));
    }

    #[test]
    fn arg_axis_is_universal_but_uses_arg_name_grammar() {
        assert!(accepts("{{arg.worktree_name}}", TemplatePosition::Prompt));
        assert!(accepts("{{arg.name}}", TemplatePosition::DeadLetterReason));
        // A hyphen can never be a declared arg name, so it is not a valid
        // template-referenceable name either.
        assert!(!accepts("{{arg.foo-bar}}", TemplatePosition::Prompt));
        // Args are flat — no nested fields.
        assert!(!accepts("{{arg.a.b}}", TemplatePosition::Prompt));
        assert!(!accepts("{{arg}}", TemplatePosition::Prompt));
    }

    #[test]
    fn malformed_placeholders_error() {
        assert!(!accepts("{{}}", TemplatePosition::Prompt));
        assert!(!accepts("{{signal.id", TemplatePosition::Prompt));
        assert!(!accepts("{{signal}}", TemplatePosition::Prompt));
    }

    #[test]
    fn today_does_not_take_a_field() {
        let mut a = Analyzer::default();
        let body = "{{today.iso}}";
        a.validate_template(body, &(0..body.len()), TemplatePosition::Prompt);
        assert_eq!(a.errors.len(), 1);
        assert!(
            a.errors[0].message.contains("does not take a field"),
            "expected a targeted `today` message, got: {}",
            a.errors[0].message
        );
    }
}
