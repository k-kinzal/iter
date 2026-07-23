//! Runner and Compose `on <event> { ... }` lowering plus Hook actions.

use std::collections::{BTreeMap, BTreeSet};

use super::{Analyzer, TemplatePosition, closest};
use crate::ast::{
    ComposeAction, ComposeEventName, ComposeHookDef, EnqueueActionDef, EventHandlerDef, EventName,
    RunnerAction, ShellActionDef, ShellCaptureDef, ShellCaptureFormat, ShellCaptureMode,
    ShellCaptureParse, ShellCaptureStream, Span, Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstAction, CstActionBody, CstBlock, CstCapture, CstField, CstIdent, CstValue};

impl Analyzer {
    pub(super) fn lower_event(
        &mut self,
        event: &CstIdent,
        body: &CstBlock,
        span: Span,
    ) -> Option<Spanned<EventHandlerDef>> {
        let event_name = if let Some((e, deprecated_alias)) =
            EventName::parse_with_deprecation(&event.name)
        {
            if let Some(alias) = deprecated_alias {
                let canonical = e.as_str();
                self.errors.push(
                    Diagnostic::warning(
                        event.span.clone(),
                        format!("event name `{alias}` is deprecated; use `{canonical}` instead",),
                    )
                    .with_hint(format!("rename `on {alias}` to `on {canonical}`")),
                );
            }
            e
        } else {
            let suggestion = closest(&event.name, EventName::ALL);
            let mut diag = Diagnostic::error(
                event.span.clone(),
                format!("unknown event name `{}`", event.name),
            );
            if let Some(s) = suggestion {
                diag = diag.with_hint(format!("did you mean `{s}`?"));
            }
            self.errors.push(diag);
            return None;
        };
        let position = if matches!(
            event_name,
            EventName::RunnerCompleting | EventName::RunnerCompleted
        ) {
            TemplatePosition::CompletionShellAction
        } else {
            TemplatePosition::ShellAction
        };
        let actions = self.lower_actions(body, position);
        if !body.fields.is_empty() {
            for f in &body.fields {
                self.errors.push(Diagnostic::error(
                    f.span.clone(),
                    format!(
                        "field `{}` is not allowed inside an event handler block",
                        f.name.name
                    ),
                ));
            }
        }
        if !body.routes.is_empty() {
            for r in &body.routes {
                self.errors.push(Diagnostic::error(
                    r.span.clone(),
                    "nested `on \"...\"` routes are only valid inside `trigger webhook`",
                ));
            }
        }
        for capture in &body.captures {
            self.errors.push(Diagnostic::error(
                capture.span.clone(),
                "`capture` is only valid inside a block-form `shell { ... }` action",
            ));
        }
        for condition in &body.conditions {
            self.errors.push(Diagnostic::error(
                condition.span.clone(),
                "completion conditions are not valid inside an event handler block",
            ));
        }
        for arm in &body.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid inside an event handler block",
            ));
        }
        for handler in &body.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "nested event handlers are not valid inside an event handler block",
            ));
        }
        Some(Spanned::new(
            EventHandlerDef {
                event: event_name,
                actions,
            },
            span,
        ))
    }

    pub(super) fn lower_actions(
        &mut self,
        block: &CstBlock,
        position: TemplatePosition,
    ) -> Vec<RunnerAction> {
        let mut out = Vec::new();
        for raw in &block.actions {
            match &raw.body {
                CstActionBody::Enqueue(_) => self.errors.push(
                    Diagnostic::error(
                        raw.span.clone(),
                        "`enqueue` actions are only valid in Compose Hooks",
                    )
                    .with_hint(
                        "publish from a top-level `on <compose-event>` block, or use a Runner Hook `shell` action",
                    ),
                ),
                CstActionBody::Shorthand { .. } | CstActionBody::Block(_) => {
                    if let Some(action) = self.lower_shell_action(raw, position) {
                        out.push(RunnerAction::Shell(action));
                    }
                }
            }
        }
        out
    }

    fn lower_shell_action(
        &mut self,
        raw: &CstAction,
        position: TemplatePosition,
    ) -> Option<ShellActionDef> {
        match &raw.body {
            CstActionBody::Shorthand {
                script,
                script_span,
            } => {
                self.validate_template(script, script_span, position);
                Some(ShellActionDef::simple(script.clone()))
            }
            CstActionBody::Block(body) => {
                let mut fields = self.collect_action_fields(&body.fields);
                let script = self.take_required_string(
                    &mut fields,
                    "script",
                    &raw.keyword_span,
                    "shell action",
                );
                self.reject_unknown_fields(&mut fields, &["script"], "shell action");
                let script = script?;
                self.validate_template(&script, &raw.span, position);
                self.reject_shell_action_nested_forms(body);

                let mut seen = BTreeSet::new();
                let mut captures = Vec::with_capacity(body.captures.len());
                for capture in &body.captures {
                    if !seen.insert(capture.name.name.clone()) {
                        self.errors.push(Diagnostic::error(
                            capture.name.span.clone(),
                            format!(
                                "duplicate capture name `{}` in shell action",
                                capture.name.name
                            ),
                        ));
                        continue;
                    }
                    captures.push(self.lower_shell_capture(capture));
                }
                if position == TemplatePosition::ComposeHookAction && !captures.is_empty() {
                    for capture in &body.captures {
                        self.errors.push(Diagnostic::error(
                            capture.span.clone(),
                            "shell capture is only available in Runner lifecycle hooks",
                        ));
                    }
                    captures.clear();
                }
                Some(ShellActionDef { script, captures })
            }
            CstActionBody::Enqueue(_) => None,
        }
    }

    fn lower_compose_actions(&mut self, block: &CstBlock) -> Vec<ComposeAction> {
        let mut out = Vec::with_capacity(block.actions.len());
        for raw in &block.actions {
            match &raw.body {
                CstActionBody::Enqueue(body) => {
                    out.push(ComposeAction::Enqueue(self.lower_enqueue_action(body)));
                }
                CstActionBody::Shorthand { .. } | CstActionBody::Block(_) => {
                    if let Some(action) =
                        self.lower_shell_action(raw, TemplatePosition::ComposeHookAction)
                    {
                        out.push(ComposeAction::Shell(action));
                    }
                }
            }
        }
        out
    }

    fn lower_enqueue_action(&mut self, body: &CstBlock) -> EnqueueActionDef {
        let mut fields = self.collect_action_fields(&body.fields);
        let target = match take_optional_ident(&mut fields, "target") {
            Ok(target) => target,
            Err((span, message)) => {
                self.errors.push(Diagnostic::error(span, message));
                None
            }
        };
        let metadata = self
            .take_optional_metadata_block(
                &mut fields,
                "metadata",
                TemplatePosition::ComposeHookAction,
            )
            .unwrap_or_default();
        let priority = self.take_optional_priority(&mut fields, "priority");
        self.reject_unknown_fields(
            &mut fields,
            &["target", "metadata", "priority"],
            "enqueue action",
        );
        self.reject_enqueue_action_nested_forms(body);
        EnqueueActionDef {
            target,
            metadata,
            priority,
        }
    }

    fn lower_shell_capture(&mut self, raw: &CstCapture) -> ShellCaptureDef {
        let mut fields = self.collect_action_fields(&raw.body.fields);
        let stream = match take_optional_ident(&mut fields, "stream") {
            Ok(Some(value)) if value == "stdout" => ShellCaptureStream::Stdout,
            Ok(Some(value)) if value == "stderr" => ShellCaptureStream::Stderr,
            Ok(Some(value)) => {
                self.errors.push(
                    Diagnostic::error(
                        raw.span.clone(),
                        format!("unknown capture stream `{value}`"),
                    )
                    .with_hint("valid streams: `stdout`, `stderr`"),
                );
                ShellCaptureStream::Stdout
            }
            Ok(None) => ShellCaptureStream::Stdout,
            Err((span, message)) => {
                self.errors.push(Diagnostic::error(span, message));
                ShellCaptureStream::Stdout
            }
        };
        let mode = match take_optional_ident(&mut fields, "mode") {
            Ok(Some(value)) if value == "replace" => ShellCaptureMode::Replace,
            Ok(Some(value)) if value == "append" => ShellCaptureMode::Append,
            Ok(Some(value)) => {
                self.errors.push(
                    Diagnostic::error(raw.span.clone(), format!("unknown capture mode `{value}`"))
                        .with_hint("valid modes: `replace`, `append`"),
                );
                ShellCaptureMode::Replace
            }
            Ok(None) => ShellCaptureMode::Replace,
            Err((span, message)) => {
                self.errors.push(Diagnostic::error(span, message));
                ShellCaptureMode::Replace
            }
        };
        let parse = fields
            .remove("parse")
            .map_or(ShellCaptureParse::Auto, |field| {
                self.lower_capture_parse(field.value)
            });
        self.reject_unknown_fields(&mut fields, &["stream", "mode", "parse"], "shell capture");
        self.reject_capture_nested_forms(&raw.body);
        ShellCaptureDef {
            name: raw.name.name.clone(),
            stream,
            mode,
            parse,
        }
    }

    fn collect_action_fields(&mut self, fields: &[CstField]) -> BTreeMap<String, CstField> {
        let mut map = BTreeMap::new();
        for field in fields {
            if map.contains_key(&field.name.name) {
                self.errors.push(Diagnostic::error(
                    field.name.span.clone(),
                    format!("duplicate field `{}` in block", field.name.name),
                ));
                continue;
            }
            map.insert(field.name.name.clone(), field.clone());
        }
        map
    }

    fn lower_capture_parse(&mut self, value: CstValue) -> ShellCaptureParse {
        match value {
            CstValue::Ident(name, _) if name == "auto" => ShellCaptureParse::Auto,
            CstValue::Ident(name, span) => {
                let format = self.lower_capture_format(&name, span);
                ShellCaptureParse::Ordered(format.into_iter().collect())
            }
            CstValue::List(values, span) => {
                if values.is_empty() {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`parse` format list must not be empty",
                    ));
                    return ShellCaptureParse::Ordered(Vec::new());
                }
                let mut seen = BTreeSet::new();
                let mut formats = Vec::with_capacity(values.len());
                for value in values {
                    match value {
                        CstValue::Ident(name, item_span) if name == "auto" => {
                            self.errors.push(
                                Diagnostic::error(
                                    item_span,
                                    "`auto` cannot be mixed into an ordered parser list",
                                )
                                .with_hint("use `parse = auto`, or list concrete formats in order"),
                            );
                        }
                        CstValue::Ident(name, item_span) => {
                            if let Some(format) = self.lower_capture_format(&name, item_span)
                                && seen.insert(format.as_str())
                            {
                                formats.push(format);
                            }
                        }
                        other => self.errors.push(Diagnostic::error(
                            other.span(),
                            "`parse` list entries must be bare format names",
                        )),
                    }
                }
                ShellCaptureParse::Ordered(formats)
            }
            other => {
                self.errors.push(
                    Diagnostic::error(
                        other.span(),
                        "`parse` must be `auto`, a format name, or an ordered format list",
                    )
                    .with_hint("use `parse = auto`, `parse = csv`, or `parse = [json, yaml]`"),
                );
                ShellCaptureParse::Auto
            }
        }
    }

    fn lower_capture_format(&mut self, name: &str, span: Span) -> Option<ShellCaptureFormat> {
        if let Some(format) = ShellCaptureFormat::parse(name) {
            return Some(format);
        }
        let mut diagnostic =
            Diagnostic::error(span, format!("unknown shell capture format `{name}`"));
        if let Some(suggestion) = closest(name, ShellCaptureFormat::ALL) {
            diagnostic = diagnostic.with_hint(format!("did you mean `{suggestion}`?"));
        } else {
            diagnostic = diagnostic.with_hint(format!(
                "valid formats: {}",
                ShellCaptureFormat::ALL.join(", ")
            ));
        }
        self.errors.push(diagnostic);
        None
    }

    fn reject_shell_action_nested_forms(&mut self, body: &CstBlock) {
        for action in &body.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "nested shell actions are not valid inside `shell { ... }`",
            ));
        }
        for route in &body.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "webhook routes are not valid inside `shell { ... }`",
            ));
        }
        for condition in &body.conditions {
            self.errors.push(Diagnostic::error(
                condition.span.clone(),
                "completion conditions are not valid inside `shell { ... }`",
            ));
        }
        for arm in &body.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid inside `shell { ... }`",
            ));
        }
        for handler in &body.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "event handlers are not valid inside `shell { ... }`",
            ));
        }
    }

    fn reject_capture_nested_forms(&mut self, body: &CstBlock) {
        for action in &body.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "shell actions are not valid inside a `capture` block",
            ));
        }
        for capture in &body.captures {
            self.errors.push(Diagnostic::error(
                capture.span.clone(),
                "nested captures are not valid inside a `capture` block",
            ));
        }
        for route in &body.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "webhook routes are not valid inside a `capture` block",
            ));
        }
        for condition in &body.conditions {
            self.errors.push(Diagnostic::error(
                condition.span.clone(),
                "completion conditions are not valid inside a `capture` block",
            ));
        }
        for arm in &body.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid inside a `capture` block",
            ));
        }
        for handler in &body.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "event handlers are not valid inside a `capture` block",
            ));
        }
    }

    fn reject_enqueue_action_nested_forms(&mut self, body: &CstBlock) {
        for action in &body.actions {
            self.errors.push(Diagnostic::error(
                action.span.clone(),
                "nested actions are not valid inside `enqueue { ... }`",
            ));
        }
        for capture in &body.captures {
            self.errors.push(Diagnostic::error(
                capture.span.clone(),
                "captures are not valid inside `enqueue { ... }`",
            ));
        }
        for route in &body.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "webhook routes are not valid inside `enqueue { ... }`",
            ));
        }
        for condition in &body.conditions {
            self.errors.push(Diagnostic::error(
                condition.span.clone(),
                "completion conditions are not valid inside `enqueue { ... }`",
            ));
        }
        for arm in &body.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid inside `enqueue { ... }`",
            ));
        }
        for handler in &body.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "event handlers are not valid inside `enqueue { ... }`",
            ));
        }
    }

    pub(super) fn lower_compose_hook(
        &mut self,
        event: &CstIdent,
        body: &CstBlock,
        span: Span,
    ) -> Option<Spanned<ComposeHookDef>> {
        let Some(event_name) = ComposeEventName::parse(&event.name) else {
            let suggestion = closest(&event.name, ComposeEventName::ALL);
            let mut diagnostic = Diagnostic::error(
                event.span.clone(),
                format!("unknown Compose event name `{}`", event.name),
            );
            if let Some(suggestion) = suggestion {
                diagnostic = diagnostic.with_hint(format!("did you mean `{suggestion}`?"));
            }
            self.errors.push(diagnostic);
            return None;
        };

        let mut services = None;
        let mut triggers = None;
        for field in &body.fields {
            match field.name.name.as_str() {
                "services" => {
                    if services.is_some() {
                        self.errors.push(Diagnostic::error(
                            field.span.clone(),
                            "duplicate `services` selector",
                        ));
                    } else {
                        services = self.lower_compose_hook_selector(&field.value, "services");
                    }
                }
                "triggers" => {
                    if triggers.is_some() {
                        self.errors.push(Diagnostic::error(
                            field.span.clone(),
                            "duplicate `triggers` selector",
                        ));
                    } else {
                        triggers = self.lower_compose_hook_selector(&field.value, "triggers");
                    }
                }
                other => self.errors.push(Diagnostic::error(
                    field.span.clone(),
                    format!("field `{other}` is not allowed inside a Compose hook"),
                )),
            }
        }

        if services.is_some() && !event_name.uses_services() {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("`services` is not valid for `{}`", event_name.as_str()),
                )
                .with_hint("remove the selector or use a service-scoped Compose event"),
            );
        }
        if triggers.is_some() && !event_name.uses_triggers() {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("`triggers` is not valid for `{}`", event_name.as_str()),
                )
                .with_hint("remove the selector or use a trigger-scoped Compose event"),
            );
        }

        let actions = self.lower_compose_actions(body);
        self.reject_compose_hook_nested_forms(body);

        Some(Spanned::new(
            ComposeHookDef {
                event: event_name,
                services,
                triggers,
                actions,
            },
            span,
        ))
    }

    fn lower_compose_hook_selector(
        &mut self,
        value: &CstValue,
        selector: &str,
    ) -> Option<Vec<String>> {
        let CstValue::List(values, span) = value else {
            self.errors.push(
                Diagnostic::error(
                    value.span(),
                    format!("`{selector}` must be a list of bare resource names"),
                )
                .with_hint(format!("write `{selector} = [name_a, name_b]`")),
            );
            return None;
        };
        if values.is_empty() {
            self.errors.push(Diagnostic::error(
                span.clone(),
                format!("`{selector}` must not be empty"),
            ));
            return None;
        }

        let mut seen = BTreeSet::new();
        let mut names = Vec::with_capacity(values.len());
        for value in values {
            match value {
                CstValue::Ident(name, value_span) => {
                    if seen.insert(name.clone()) {
                        names.push(name.clone());
                    } else {
                        self.errors.push(Diagnostic::error(
                            value_span.clone(),
                            format!("duplicate resource name `{name}` in `{selector}`"),
                        ));
                    }
                }
                other => self.errors.push(
                    Diagnostic::error(
                        other.span(),
                        format!("`{selector}` entries must be bare resource names"),
                    )
                    .with_hint(format!("write `{selector} = [name_a, name_b]`")),
                ),
            }
        }
        Some(names)
    }

    fn reject_compose_hook_nested_forms(&mut self, body: &CstBlock) {
        for capture in &body.captures {
            self.errors.push(Diagnostic::error(
                capture.span.clone(),
                "`capture` is only valid inside a block-form `shell { ... }` action",
            ));
        }
        for route in &body.routes {
            self.errors.push(Diagnostic::error(
                route.span.clone(),
                "nested webhook routes are not valid inside a Compose hook",
            ));
        }
        for condition in &body.conditions {
            self.errors.push(Diagnostic::error(
                condition.span.clone(),
                "completion conditions are not valid inside a Compose hook",
            ));
        }
        for arm in &body.prompt_arms {
            self.errors.push(Diagnostic::error(
                arm.span.clone(),
                "prompt match arms are not valid inside a Compose hook",
            ));
        }
        for handler in &body.event_handlers {
            self.errors.push(Diagnostic::error(
                handler.span.clone(),
                "nested event handlers are not valid inside a Compose hook",
            ));
        }
    }
}

fn take_optional_ident(
    fields: &mut BTreeMap<String, CstField>,
    name: &str,
) -> Result<Option<String>, (Span, String)> {
    let Some(field) = fields.remove(name) else {
        return Ok(None);
    };
    match field.value {
        CstValue::Ident(value, _) => Ok(Some(value)),
        other => Err((other.span(), format!("`{name}` must be a bare identifier"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::lower_and_check;
    use crate::diagnostic::Severity;
    use crate::parse_to_cst;
    use crate::{
        RunnerAction, ShellCaptureFormat, ShellCaptureMode, ShellCaptureParse, ShellCaptureStream,
    };

    /// A minimal Iterfile head: every required section plus a runner that
    /// binds its definitions, left open (no closing brace) so a test can
    /// append runner-scoped `on` blocks. The file as a whole validates, so
    /// the only diagnostics that survive are the ones we want to inspect.
    const HEAD: &str = r#"
queue memory
workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back {
    mode = sync
  }
}
agent claude {
  mode = print
  command = "claude"
}
runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = false
  behavior = wait
  prompt = "Iterate."
"#;

    /// Close [`HEAD`] around `on_blocks`, yielding a complete Iterfile whose
    /// runner carries the given event handlers.
    fn iterfile(on_blocks: &str) -> String {
        format!("{HEAD}\n{on_blocks}}}\n")
    }

    fn analyze(src: &str) -> Vec<crate::Diagnostic> {
        let (cst, mut diags) = parse_to_cst(src);
        let cst = cst.expect("parser produced a CST");
        let (_root, sem) = lower_and_check(cst);
        diags.extend(sem);
        diags
    }

    #[test]
    fn deprecated_alias_emits_one_warning_with_canonical_hint() {
        let src = iterfile("on workspace_torndown { shell \"echo done\" }\n");
        let diags = analyze(&src);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warning for one alias use; got {diags:?}"
        );
        let w = warnings[0];
        assert!(
            w.message.contains("workspace_torndown"),
            "warning names the alias: {}",
            w.message
        );
        assert!(
            w.message.contains("workspace_teardown_finished"),
            "warning recommends the canonical: {}",
            w.message
        );
        let hint = w.hint.as_deref().unwrap_or("");
        assert!(
            hint.contains("workspace_teardown_finished"),
            "hint steers to canonical: {hint}"
        );

        // Span check: the warning should point at the alias token, not
        // the whole `on` block. We assert the slice equals the alias.
        let span = w.span.clone();
        assert_eq!(&src[span], "workspace_torndown");
    }

    #[test]
    fn canonical_event_name_emits_no_warning() {
        let src = iterfile("on workspace_teardown_finished { shell \"echo done\" }\n");
        let diags = analyze(&src);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(
            warnings.is_empty(),
            "canonical name must not warn; got {warnings:?}"
        );
    }

    #[test]
    fn each_deprecated_alias_warns_separately() {
        // Multiple aliases in one file: each should produce its own
        // warning with the corresponding canonical recommendation.
        let cases = [
            ("workspace_setting_up", "workspace_setup_starting"),
            ("workspace_set_up", "workspace_setup_finished"),
            ("workspace_tearing_down", "workspace_teardown_starting"),
            ("workspace_torndown", "workspace_teardown_finished"),
        ];
        let mut body = String::new();
        for (alias, _) in cases {
            use std::fmt::Write as _;
            writeln!(body, "on {alias} {{ shell \"echo {alias}\" }}").expect("write to String");
        }
        let src = iterfile(&body);
        let diags = analyze(&src);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(warnings.len(), cases.len(), "one warning per alias");
        for (alias, canonical) in cases {
            assert!(
                warnings
                    .iter()
                    .any(|w| w.message.contains(alias) && w.message.contains(canonical)),
                "warning for `{alias}` -> `{canonical}` missing in {warnings:?}"
            );
        }
    }

    #[test]
    fn unknown_event_name_is_an_error_not_a_warning() {
        let src = iterfile("on not_a_real_event { shell \"echo x\" }\n");
        let diags = analyze(&src);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "unknown event name must error; diagnostics: {diags:?}"
        );
        // And critically: the unknown-name spell-check must not steer
        // the user toward a deprecated alias.
        for e in &errors {
            let hint = e.hint.as_deref().unwrap_or("");
            for alias in [
                "workspace_setting_up",
                "workspace_set_up",
                "workspace_tearing_down",
                "workspace_torndown",
            ] {
                assert!(
                    !hint.contains(alias),
                    "spell-check hint must not point at deprecated alias `{alias}`: {hint}"
                );
            }
        }
    }

    #[test]
    fn block_shell_capture_lowers_to_typed_definition() {
        let src = iterfile(
            r#"
on runner_starting {
  shell {
    script = "printf '{\"foo\":1}'"
    capture context {
      stream = stderr
      mode = append
      parse = [json, yaml, csv]
    }
  }
}
"#,
        );
        let root = crate::parse(&src).expect("valid block shell");
        let action = &root.runners[0].node.events[0].node.actions[0];
        let RunnerAction::Shell(definition) = action;
        assert_eq!(definition.script, "printf '{\"foo\":1}'");
        assert_eq!(definition.captures.len(), 1);
        let capture = &definition.captures[0];
        assert_eq!(capture.name, "context");
        assert_eq!(capture.stream, ShellCaptureStream::Stderr);
        assert_eq!(capture.mode, ShellCaptureMode::Append);
        assert_eq!(
            capture.parse,
            ShellCaptureParse::Ordered(vec![
                ShellCaptureFormat::Json,
                ShellCaptureFormat::Yaml,
                ShellCaptureFormat::Csv,
            ])
        );
    }

    #[test]
    fn capture_defaults_are_stdout_replace_auto() {
        let src = iterfile(
            r#"
on runner_starting {
  shell {
    script = "printf x"
    capture context {}
  }
}
"#,
        );
        let root = crate::parse(&src).expect("valid default capture");
        let RunnerAction::Shell(definition) = &root.runners[0].node.events[0].node.actions[0];
        let capture = &definition.captures[0];
        assert_eq!(capture.stream, ShellCaptureStream::Stdout);
        assert_eq!(capture.mode, ShellCaptureMode::Replace);
        assert_eq!(capture.parse, ShellCaptureParse::Auto);
    }

    #[test]
    fn capture_rejects_unknown_format_and_duplicate_name() {
        let src = iterfile(
            r#"
on runner_starting {
  shell {
    script = "printf x"
    capture context { parse = jsoon }
    capture context { parse = auto }
  }
}
"#,
        );
        let diagnostics = analyze(&src);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("jsoon")),
            "unknown format diagnostic missing: {diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate capture name")),
            "duplicate-name diagnostic missing: {diagnostics:?}"
        );
    }

    #[test]
    fn capture_rejects_invalid_stream_mode_and_parser_lists() {
        let src = iterfile(
            r#"
on runner_starting {
  shell {
    script = "printf x"
    capture first {
      stream = output
      mode = merge
      parse = []
    }
    capture second {
      parse = [auto, json]
    }
  }
}
"#,
        );
        let diagnostics = analyze(&src);
        for expected in [
            "unknown capture stream",
            "unknown capture mode",
            "must not be empty",
            "`auto` cannot be mixed",
        ] {
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "missing `{expected}` diagnostic: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn block_shell_requires_script() {
        let src = iterfile(
            r"
on runner_starting {
  shell {
    capture context {}
  }
}
",
        );
        let diagnostics = analyze(&src);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("requires `script`")),
            "missing required-script diagnostic: {diagnostics:?}"
        );
    }
}
