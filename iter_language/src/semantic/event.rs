//! Runner and Compose `on <event> { ... }` lowering plus shell actions.

use std::collections::BTreeSet;

use super::{Analyzer, TemplatePosition, closest};
use crate::ast::{
    Action, ComposeEventName, ComposeHookDef, EventHandlerDef, EventName, Span, Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstAction, CstBlock, CstIdent, CstValue};

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
    ) -> Vec<Action> {
        let mut out = Vec::new();
        for raw in &block.actions {
            let CstAction { command, .. } = raw;
            self.validate_template(command, &raw.span, position);
            out.push(Action::Shell(command.clone()));
        }
        out
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

        let actions = self.lower_actions(body, TemplatePosition::ComposeShellAction);
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

#[cfg(test)]
mod tests {
    use super::super::lower_and_check;
    use crate::diagnostic::Severity;
    use crate::parse_to_cst;

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
}
