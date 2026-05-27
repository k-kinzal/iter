//! `runner { ... }` lowerer.

use std::collections::BTreeMap;

use super::{Analyzer, CONTINUE_ON_ERROR_HINT, RUNNER_BEHAVIOR_HINT};
use crate::ast::{
    EventHandlerDecl, PromptExpr, PromptValue, RunnerBehavior, RunnerDecl, Span, Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawIdent, RawValue};

impl Analyzer {
    /// Lower old-style `runner { continue_on_error = ... behavior = ... }`.
    /// Returns a partial struct with only the runtime policy fields set;
    /// the caller fills in agent/workspace/queue/prompt/events.
    pub(super) fn lower_runner_old(
        &mut self,
        kind: Option<&RawIdent>,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<OldRunnerDecl> {
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
        Some(OldRunnerDecl {
            continue_on_error: continue_on_error?,
            behavior: behavior?,
            iteration_timeout_secs,
        })
    }

    /// Lower new-style `runner { agent = <ref> workspace = <ref> ... }`.
    pub(super) fn lower_runner_new(
        &mut self,
        kind: Option<&RawIdent>,
        alias: Option<RawIdent>,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<RunnerDecl> {
        if let Some(kind) = kind {
            self.errors.push(Diagnostic::error(
                kind.span.clone(),
                format!("`runner` takes no kind, found `{}`", kind.name),
            ));
        }
        let name = alias.map(|a| a.name);

        let Some(block) = body else {
            self.errors.push(Diagnostic::error(
                keyword_span.clone(),
                "runner requires a body",
            ));
            return None;
        };

        // Separate out `on <event> { ... }` actions and prompt block/field.
        let mut regular_fields = Vec::new();
        let mut events: Vec<Spanned<EventHandlerDecl>> = Vec::new();

        for field in block.fields {
            regular_fields.push(field);
        }
        // Handle nested `on <event>` blocks from the raw block's actions/routes.
        // In the current parser, `on` inside blocks appears as routes.
        // Event handlers in runner body are parsed by the top-level `on` parser,
        // so they appear as top-level sections. For now, runner events must be
        // top-level `on` sections (handled by caller) or nested fields.
        // We'll look for `on_*` fields as a workaround, but the preferred
        // path is top-level `on` attached by the caller.
        let _ = &mut events;

        let mut fields = self.collect_fields_from_vec(regular_fields);

        // Extract binding references.
        let agent = self.take_required_ident(&mut fields, "agent", keyword_span, "runner");
        let workspace = self.take_required_ident(&mut fields, "workspace", keyword_span, "runner");
        let queue = self.take_optional_ident(&mut fields, "queue");

        let continue_on_error = self.take_required_bool_explicit(
            &mut fields,
            "continue_on_error",
            keyword_span,
            "runner",
            CONTINUE_ON_ERROR_HINT,
        );
        let behavior = self.take_required_runner_behavior(&mut fields, keyword_span);
        let iteration_timeout_secs = self.take_iteration_timeout_secs(&mut fields);

        // Parse prompt expression.
        let prompt = self.take_prompt_expr(&mut fields, keyword_span);

        self.reject_unknown_fields(
            &mut fields,
            &[
                "agent",
                "workspace",
                "queue",
                "continue_on_error",
                "behavior",
                "iteration_timeout_secs",
                "prompt",
            ],
            "runner",
        );

        Some(RunnerDecl {
            name,
            agent: agent?,
            workspace: workspace?,
            queue,
            continue_on_error: continue_on_error?,
            behavior: behavior?,
            iteration_timeout_secs,
            prompt,
            events,
        })
    }

    /// Extract a required identifier field (bareword reference).
    fn take_required_ident(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        name: &str,
        keyword_span: &Span,
        context: &str,
    ) -> Option<String> {
        if let Some(field) = fields.remove(name) {
            match field.value {
                RawValue::Ident(s, _) | RawValue::String(s, _) => Some(s),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        format!("`{name}` must be an identifier reference"),
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(
                    keyword_span.clone(),
                    format!("{context} requires `{name}`"),
                )
                .with_hint(format!("add `{name} = <reference>`")),
            );
            None
        }
    }

    /// Extract an optional identifier field.
    fn take_optional_ident(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        name: &str,
    ) -> Option<String> {
        let field = fields.remove(name)?;
        match field.value {
            RawValue::Ident(s, _) | RawValue::String(s, _) => Some(s),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be an identifier reference"),
                ));
                None
            }
        }
    }

    /// Parse the `prompt` field/block inside a runner:
    /// - `prompt = "text"` → Single(Inline)
    /// - `prompt = name` → Single(Ref)
    /// - `prompt { _ = "default" }` → Single(default or empty)
    fn take_prompt_expr(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        keyword_span: &Span,
    ) -> PromptExpr {
        let Some(field) = fields.remove("prompt") else {
            self.errors.push(
                Diagnostic::error(keyword_span.clone(), "runner requires `prompt`")
                    .with_hint("add `prompt = \"...\"` or a prompt match block"),
            );
            return PromptExpr::Single(PromptValue::Inline(String::new()));
        };

        match field.value {
            RawValue::String(s, span) => {
                self.validate_template(&s, &span);
                PromptExpr::Single(PromptValue::Inline(s))
            }
            RawValue::Ident(name, _) => PromptExpr::Single(PromptValue::Ref(name)),
            RawValue::Block(block) => self.parse_prompt_match_block(block),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`prompt` must be a string, a name reference, or a match block",
                ));
                PromptExpr::Single(PromptValue::Inline(String::new()))
            }
        }
    }

    /// Parse `prompt { guard => value, ... _ => default }`.
    ///
    /// Guard expressions in block syntax are not yet supported — the parser
    /// would need `=>` as a token and a guard-expression sub-parser. For now
    /// only the `_ = "default"` arm is accepted; all other fields emit an error.
    fn parse_prompt_match_block(&mut self, block: RawBlock) -> PromptExpr {
        let mut default: Option<PromptValue> = None;

        for field in block.fields {
            let value = match field.value {
                RawValue::String(s, span) => {
                    self.validate_template(&s, &span);
                    PromptValue::Inline(s)
                }
                RawValue::Ident(name, _) => PromptValue::Ref(name),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "prompt match arm value must be a string or a name reference",
                    ));
                    continue;
                }
            };

            if field.name.name == "_" {
                if default.is_some() {
                    self.errors.push(Diagnostic::error(
                        field.name.span,
                        "duplicate default arm `_` in prompt match",
                    ));
                } else {
                    default = Some(value);
                }
            } else {
                self.errors.push(Diagnostic::error(
                    field.name.span,
                    "prompt match guard expressions are not yet supported; use `prompt = \"...\"` for now",
                ));
            }
        }

        PromptExpr::Single(default.unwrap_or(PromptValue::Inline(String::new())))
    }

    fn collect_fields_from_vec(
        &mut self,
        fields: Vec<RawField>,
    ) -> BTreeMap<String, RawField> {
        let mut map = BTreeMap::new();
        for field in fields {
            if map.contains_key(&field.name.name) {
                self.errors.push(Diagnostic::error(
                    field.name.span.clone(),
                    format!("duplicate field `{}` in block", field.name.name),
                ));
                continue;
            }
            map.insert(field.name.name.clone(), field);
        }
        map
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

/// Intermediate struct for old-style runner fields (no references).
pub(super) struct OldRunnerDecl {
    pub continue_on_error: bool,
    pub behavior: RunnerBehavior,
    pub iteration_timeout_secs: Option<i64>,
}
