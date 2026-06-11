//! `trigger webhook` route lowering (the per-route `on "..." { ... }` blocks).

use std::collections::BTreeMap;

use super::{Analyzer, TemplatePosition};
use crate::ast::Subscription;
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstRoute};

impl Analyzer {
    pub(super) fn lower_webhook_routes(&mut self, routes: &[CstRoute]) -> Vec<Subscription> {
        let mut out = Vec::new();
        for route in routes {
            let block = CstBlock {
                fields: route.body.fields.clone(),
                routes: Vec::new(),
                actions: Vec::new(),
                prompt_arms: Vec::new(),
                event_handlers: Vec::new(),
                span: route.body.span.clone(),
            };
            let mut fields: BTreeMap<String, CstField> = self.collect_fields(Some(block));
            // The `when` guard and per-subscription metadata are the two
            // positions where `{{event.*}}` is legal (R8). The guard's
            // `{{...}}` placeholders are validated here; the full guard
            // grammar (and fail-closed evaluation) is reworked when the
            // webhook source becomes a subprocess.
            if let Some(when) = &route.when {
                let when_span = route
                    .when_span
                    .clone()
                    .unwrap_or_else(|| route.span.clone());
                self.validate_template(when, &when_span, TemplatePosition::WebhookSubscription);
            }
            let priority = self.take_optional_priority(&mut fields, "priority");
            let metadata = self
                .take_optional_metadata_block(
                    &mut fields,
                    "metadata",
                    TemplatePosition::WebhookSubscription,
                )
                .unwrap_or_default();
            self.reject_unknown_fields(&mut fields, &["priority", "metadata"], "webhook route");
            for action in &route.body.actions {
                self.errors.push(Diagnostic::error(
                    action.span.clone(),
                    "`shell` actions are not allowed inside webhook routes",
                ));
            }
            for nested in &route.body.routes {
                self.errors.push(Diagnostic::error(
                    nested.span.clone(),
                    "webhook routes cannot themselves contain nested `on \"...\"` blocks",
                ));
            }
            for arm in &route.body.prompt_arms {
                self.errors.push(Diagnostic::error(
                    arm.span.clone(),
                    "prompt match arms are not valid inside webhook routes",
                ));
            }
            for handler in &route.body.event_handlers {
                self.errors.push(Diagnostic::error(
                    handler.span.clone(),
                    "event handlers are not valid inside webhook routes",
                ));
            }
            out.push(Subscription {
                event_pattern: route.event_pattern.clone(),
                when: route.when.clone(),
                priority,
                metadata,
            });
        }
        out
    }
}
