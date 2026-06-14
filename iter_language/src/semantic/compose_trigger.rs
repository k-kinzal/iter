use std::collections::BTreeMap;

use super::Analyzer;
use crate::ast::{Compose, NamedTrigger, QueueRef, Span, Spanned, TriggerDef};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstIdent, CstValue};

use super::compose::{ComposeSectionParts, TRIGGER_REQUIRES_KIND_HINT};

impl Analyzer {
    pub(super) fn lower_compose_trigger(
        &mut self,
        root: &mut Compose,
        trigger_names: &mut BTreeMap<String, Span>,
        parts: ComposeSectionParts,
    ) {
        let ComposeSectionParts {
            kind,
            kind2,
            body,
            keyword_span,
            span,
        } = parts;
        let Some((name_ident, kind_ident)) =
            self.compose_name_and_kind(kind, kind2, &keyword_span, "trigger")
        else {
            return;
        };
        let Some(kind_ident) = kind_ident else {
            self.errors.push(
                Diagnostic::error(name_ident.span, "compose.iter `trigger` requires a kind")
                    .with_hint(TRIGGER_REQUIRES_KIND_HINT),
            );
            return;
        };
        if matches!(kind_ident.name.as_str(), "loop") {
            self.errors.push(
                Diagnostic::error(kind_ident.span, "`loop` is no longer a trigger kind")
                    .with_hint(
                        "use `runner { behavior = loop { delay_secs = N } }` inside the service Iterfile instead.",
                    ),
            );
            return;
        }
        if let Some(prev) = trigger_names.get(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate trigger name `{}`", name_ident.name),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end
                )),
            );
            return;
        }
        trigger_names.insert(name_ident.name.clone(), span.clone());
        let target = take_target_field(body.as_ref(), &mut self.errors);
        let terminate_on_completion =
            take_bool_field(body.as_ref(), "terminate_on_completion", &mut self.errors);
        if let Some(decl) = self.lower_trigger_with_target(kind_ident, body, &keyword_span) {
            root.triggers.push(Spanned::new(
                NamedTrigger {
                    name: name_ident.name,
                    decl,
                    target,
                    terminate_on_completion,
                },
                span,
            ));
        }
    }

    pub(super) fn lower_trigger_with_target(
        &mut self,
        kind: CstIdent,
        body: Option<CstBlock>,
        keyword_span: &Span,
    ) -> Option<TriggerDef> {
        let body = body.map(|b| CstBlock {
            fields: b
                .fields
                .into_iter()
                .filter(|f| f.name.name != "target" && f.name.name != "terminate_on_completion")
                .collect(),
            routes: b.routes,
            actions: b.actions,
            prompt_arms: b.prompt_arms,
            event_handlers: b.event_handlers,
            span: b.span,
        });
        self.lower_trigger(Some(kind), body, keyword_span)
    }
}

fn take_target_field(body: Option<&CstBlock>, errors: &mut Vec<Diagnostic>) -> QueueRef {
    let Some(block) = body else {
        return QueueRef::Anonymous;
    };
    for field in &block.fields {
        if field.name.name == "target" {
            match &field.value {
                CstValue::Ident(name, _) => return QueueRef::Named(name.clone()),
                other @ (CstValue::String(..)
                | CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Bool(..)
                | CstValue::Null(_)
                | CstValue::List(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
                    errors.push(Diagnostic::error(
                        other.span(),
                        "`target` must be a queue name",
                    ));
                    return QueueRef::Anonymous;
                }
            }
        }
    }
    QueueRef::Anonymous
}

fn take_bool_field(body: Option<&CstBlock>, name: &str, errors: &mut Vec<Diagnostic>) -> bool {
    let Some(block) = body else {
        return false;
    };
    for field in &block.fields {
        if field.name.name == name {
            match &field.value {
                CstValue::Bool(val, _) => return *val,
                other @ (CstValue::String(..)
                | CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Null(_)
                | CstValue::Ident(..)
                | CstValue::List(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
                    errors.push(Diagnostic::error(
                        other.span(),
                        format!("`{name}` must be a boolean"),
                    ));
                    return false;
                }
            }
        }
    }
    false
}
