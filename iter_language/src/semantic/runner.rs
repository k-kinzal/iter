//! `runner { ... }` lowerer.

use std::collections::BTreeMap;

use super::{Analyzer, CONTINUE_ON_ERROR_HINT, RUNNER_BEHAVIOR_HINT, TemplatePosition};
use crate::ast::{
    EventHandlerDecl, PromptArm, PromptExpr, PromptValue, RunnerBehavior, RunnerDecl, Span,
    Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawIdent, RawValue};

impl Analyzer {
    /// Lower a `runner { agent = <ref> workspace = <ref> ... }` block.
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

        let mut events: Vec<Spanned<EventHandlerDecl>> = Vec::new();

        for route in &block.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "nested `on \"...\"` routes are not valid in a runner block",
            ));
        }
        for action in &block.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "`shell` actions are not valid directly in a runner block; use `on <event> { shell \"...\" }`",
            ));
        }
        for arm in &block.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid directly in a runner block; use `prompt { guard => \"...\" }`",
            ));
        }

        // Lower nested `on <event> { ... }` event handlers from the block.
        for handler in block.event_handlers {
            if let Some(decl) = self.lower_event(&handler.event, &handler.body, handler.span) {
                events.push(decl);
            }
        }

        let mut fields = self.collect_fields_from_vec(block.fields);

        // A `runner` that binds neither `agent` nor `workspace` is the legacy
        // flat Iterfile shape — those references used to be synthesised from
        // the sole top-level definitions. That desugaring is gone, so name the
        // replacement grammar in one actionable error rather than emitting two
        // generic "runner requires ..." diagnostics.
        if !fields.contains_key("agent") && !fields.contains_key("workspace") {
            self.errors.push(Diagnostic::error(
                keyword_span.clone(),
                "flat Iterfile syntax is no longer supported; define named `agent`/`workspace`/`queue` definitions and bind them in a `runner { agent = ... workspace = ... }` block",
            ));
            return None;
        }

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

    /// Lower a compose inline-service `runner { ... }` block.
    ///
    /// Unlike the Iterfile [`lower_runner_new`](Self::lower_runner_new),
    /// agent and workspace are **not** referenced here — an inline service
    /// declares them as sibling `agent_*` / `workspace_*` sub-blocks, and
    /// the compose service builder supplies those declarations directly. This
    /// lowerer therefore captures only the runner's own concerns: runtime
    /// policy, the prompt expression, and nested `on <event>` handlers,
    /// leaving the agent/workspace/queue reference fields empty.
    ///
    /// This is what routes prompt and event data through `RunnerDecl` for
    /// inline services, matching the new Iterfile design where the runner
    /// binds prompt and lifecycle events rather than carrying them as
    /// independent top-level sections.
    pub(super) fn lower_runner_inline(
        &mut self,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<RunnerDecl> {
        let Some(block) = body else {
            self.errors.push(Diagnostic::error(
                keyword_span.clone(),
                "runner requires a body",
            ));
            return None;
        };

        let mut events: Vec<Spanned<EventHandlerDecl>> = Vec::new();

        for route in &block.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "nested `on \"...\"` routes are not valid in a runner block",
            ));
        }
        for action in &block.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "`shell` actions are not valid directly in a runner block; use `on <event> { shell \"...\" }`",
            ));
        }
        for arm in &block.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid directly in a runner block; use `prompt { guard => \"...\" }`",
            ));
        }

        // Lower nested `on <event> { ... }` event handlers from the block.
        for handler in block.event_handlers {
            if let Some(decl) = self.lower_event(&handler.event, &handler.body, handler.span) {
                events.push(decl);
            }
        }

        let mut fields = self.collect_fields_from_vec(block.fields);

        let continue_on_error = self.take_required_bool_explicit(
            &mut fields,
            "continue_on_error",
            keyword_span,
            "runner",
            CONTINUE_ON_ERROR_HINT,
        );
        let behavior = self.take_required_runner_behavior(&mut fields, keyword_span);
        let iteration_timeout_secs = self.take_iteration_timeout_secs(&mut fields);
        let prompt = self.take_prompt_expr(&mut fields, keyword_span);
        // Inline services have no named-prompt scope (`compose.iter` has no
        // `prompt as <name>` construct), so a bareword prompt reference can
        // never resolve. Reject it here with a clear diagnostic rather than
        // letting it silently lower to an empty prompt downstream.
        self.reject_prompt_refs_inline(&prompt, keyword_span);

        self.reject_unknown_fields(
            &mut fields,
            &[
                "continue_on_error",
                "behavior",
                "iteration_timeout_secs",
                "prompt",
            ],
            "runner",
        );

        Some(RunnerDecl {
            name: None,
            agent: String::new(),
            workspace: String::new(),
            queue: None,
            continue_on_error: continue_on_error?,
            behavior: behavior?,
            iteration_timeout_secs,
            prompt,
            events,
        })
    }

    /// Reject `PromptValue::Ref` arms in an inline-service prompt expression.
    ///
    /// Named prompt references are an Iterfile-only feature: they resolve
    /// against the file's `prompt as <name>` definitions, which do not exist
    /// in a `compose.iter` inline service. Without this guard a bareword
    /// `prompt = name` would lower to a dangling `Ref` and silently resolve
    /// to an empty prompt at build time.
    fn reject_prompt_refs_inline(&mut self, expr: &PromptExpr, keyword_span: &Span) {
        let mut check = |value: &PromptValue| {
            if let PromptValue::Ref(name) = value {
                self.errors.push(
                    Diagnostic::error(
                        keyword_span.clone(),
                        format!(
                            "named prompt reference `{name}` is not valid in an inline service runner"
                        ),
                    )
                    .with_hint("inline services have no named prompts; write the prompt inline as `prompt = \"...\"`"),
                );
            }
        };
        match expr {
            PromptExpr::Single(value) => check(value),
            PromptExpr::Match { arms, default } => {
                for arm in arms {
                    check(&arm.value);
                }
                check(default);
            }
        }
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
                self.validate_template(&s, &span, TemplatePosition::Prompt);
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
    fn parse_prompt_match_block(&mut self, block: RawBlock) -> PromptExpr {
        // Legacy fallback: if the block has fields but no prompt_arms, accept
        // `_ = "default"` for backward compatibility.
        if block.prompt_arms.is_empty() {
            return self.parse_prompt_match_block_legacy(block);
        }

        let mut arms = Vec::new();
        let mut default: Option<PromptValue> = None;

        for arm in block.prompt_arms {
            let value = match arm.value {
                RawValue::String(s, span) => {
                    self.validate_template(&s, &span, TemplatePosition::Prompt);
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

            match arm.guard {
                None => {
                    if default.is_some() {
                        self.errors.push(Diagnostic::error(
                            arm.span,
                            "duplicate default arm `_` in prompt match",
                        ));
                    } else {
                        default = Some(value);
                    }
                }
                Some(raw_guard) => {
                    let guard = self.lower_guard(raw_guard);
                    arms.push(PromptArm { guard, value });
                }
            }
        }

        // Reject anything that isn't a prompt arm in the match block.
        for field in &block.fields {
            self.errors.push(Diagnostic::error(
                field.span.clone(),
                "unexpected field in prompt match block; use `guard => value` arms",
            ));
        }
        for route in &block.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "nested routes are not valid inside a prompt match block",
            ));
        }
        for action in &block.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "`shell` actions are not valid inside a prompt match block",
            ));
        }
        for handler in &block.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "event handlers are not valid inside a prompt match block",
            ));
        }

        let Some(default) = default else {
            self.errors.push(Diagnostic::error(
                block.span.clone(),
                "prompt match block requires a default arm (`_ => \"...\"` or `_ => name`)",
            ));
            return PromptExpr::Single(PromptValue::Inline(String::new()));
        };

        if arms.is_empty() {
            PromptExpr::Single(default)
        } else {
            PromptExpr::Match { arms, default }
        }
    }

    /// Legacy prompt match block: `prompt { _ = "default" }` using `=` syntax.
    fn parse_prompt_match_block_legacy(&mut self, block: RawBlock) -> PromptExpr {
        for route in &block.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "nested routes are not valid inside a prompt match block",
            ));
        }
        for action in &block.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "`shell` actions are not valid inside a prompt match block",
            ));
        }
        for handler in &block.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "event handlers are not valid inside a prompt match block",
            ));
        }

        let mut default: Option<PromptValue> = None;

        for field in block.fields {
            let value = match field.value {
                RawValue::String(s, span) => {
                    self.validate_template(&s, &span, TemplatePosition::Prompt);
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
                    format!(
                        "unknown field `{}` in prompt match block; use `guard => value` arms with `=>` syntax",
                        field.name.name,
                    ),
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
