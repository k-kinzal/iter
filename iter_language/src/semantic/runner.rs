//! `runner { ... }` lowerer.

use std::collections::BTreeMap;

use super::{Analyzer, CONTINUE_ON_ERROR_HINT, RUNNER_BEHAVIOR_HINT};
use crate::ast::{RunnerBehavior, RunnerDecl, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawIdent, RawValue};

impl Analyzer {
    pub(super) fn lower_runner(
        &mut self,
        kind: Option<&RawIdent>,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<RunnerDecl> {
        if let Some(kind) = kind {
            self.errors.push(Diagnostic::error(
                kind.span.clone(),
                format!("`runner` takes no kind, found `{}`", kind.name),
            ));
        }
        let mut fields = self.collect_fields(body);
        let continue_on_error = self.take_required_bool_explicit(
            &mut fields,
            "continue_on_error",
            keyword_span,
            "runner",
            CONTINUE_ON_ERROR_HINT,
        );
        let behavior = self.take_required_runner_behavior(&mut fields, keyword_span);
        let iteration_timeout_secs = self.take_iteration_timeout_secs(&mut fields);
        self.reject_unknown_fields(
            &mut fields,
            &["continue_on_error", "behavior", "iteration_timeout_secs"],
            "runner",
        );
        Some(RunnerDecl {
            continue_on_error: continue_on_error?,
            behavior: behavior?,
            iteration_timeout_secs,
        })
    }

    fn take_iteration_timeout_secs(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<i64> {
        let span = fields.get("iteration_timeout_secs")?.value.span();
        let secs = self.take_optional_duration(fields, "iteration_timeout_secs")?;
        if secs <= 0 {
            self.errors.push(Diagnostic::error(
                span,
                "`iteration_timeout_secs` must be a positive duration",
            ));
            return None;
        }
        Some(secs)
    }

    fn take_required_runner_behavior(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        keyword_span: &Span,
    ) -> Option<RunnerBehavior> {
        if let Some(field) = fields.remove("behavior") {
            match field.value {
                RawValue::Ident(name, span) => self.parse_runner_behavior_ident(&name, &span),
                RawValue::Block(block) => self.parse_runner_behavior_block(block),
                other => {
                    self.errors.push(
                    Diagnostic::error(
                        other.span(),
                        "`behavior` must be `wait`, `loop`, or `behavior { kind = ..., delay_secs = ... }`",
                    )
                    .with_hint(RUNNER_BEHAVIOR_HINT),
                );
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(keyword_span.clone(), "runner requires `behavior`")
                    .with_hint(RUNNER_BEHAVIOR_HINT),
            );
            None
        }
    }

    fn parse_runner_behavior_ident(&mut self, name: &str, span: &Span) -> Option<RunnerBehavior> {
        match name {
            "wait" => Some(RunnerBehavior::Wait),
            "loop" => Some(RunnerBehavior::Loop { delay_secs: None }),
            other => {
                self.errors.push(
                    Diagnostic::error(span.clone(), format!("unknown runner behavior `{other}`"))
                        .with_hint(RUNNER_BEHAVIOR_HINT),
                );
                None
            }
        }
    }

    fn parse_runner_behavior_block(&mut self, body: RawBlock) -> Option<RunnerBehavior> {
        let body_span = body.span.clone();
        let mut inner = self.collect_fields(Some(body));
        let kind_field = inner.remove("kind");
        let kind = if let Some(field) = kind_field {
            match field.value {
                RawValue::Ident(name, span) => Some((name, span)),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`behavior.kind` must be an identifier (`wait` or `loop`)",
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(body_span, "behavior block requires `kind`")
                    .with_hint(RUNNER_BEHAVIOR_HINT),
            );
            None
        };
        let delay_secs = self.take_optional_duration(&mut inner, "delay_secs");
        self.reject_unknown_fields(&mut inner, &["kind", "delay_secs"], "behavior");
        let (name, span) = kind?;
        match name.as_str() {
            "wait" => {
                if delay_secs.is_some() {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`behavior = wait` does not accept `delay_secs`",
                    ));
                    return None;
                }
                Some(RunnerBehavior::Wait)
            }
            "loop" => Some(RunnerBehavior::Loop { delay_secs }),
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown runner behavior `{other}`"))
                        .with_hint(RUNNER_BEHAVIOR_HINT),
                );
                None
            }
        }
    }
}
