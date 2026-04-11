//! `trigger webhook` route lowering (the per-route `on "..." { ... }` blocks).

use std::collections::BTreeMap;

use super::Analyzer;
use crate::ast::WebhookRoute;
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawRoute};

impl Analyzer {
    pub(super) fn lower_webhook_routes(&mut self, routes: &[RawRoute]) -> Vec<WebhookRoute> {
        let mut out = Vec::new();
        for route in routes {
            let block = RawBlock {
                fields: route.body.fields.clone(),
                routes: Vec::new(),
                actions: Vec::new(),
                span: route.body.span.clone(),
            };
            let mut fields: BTreeMap<String, RawField> = self.collect_fields(Some(block));
            let priority = self.take_optional_priority(&mut fields, "priority");
            let metadata = self
                .take_optional_metadata_block(&mut fields, "metadata")
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
            out.push(WebhookRoute {
                event_pattern: route.event_pattern.clone(),
                when: route.when.clone(),
                priority,
                metadata,
            });
        }
        out
    }
}
