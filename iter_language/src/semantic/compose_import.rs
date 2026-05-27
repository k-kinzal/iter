use std::collections::BTreeMap;
use std::path::PathBuf;

use super::Analyzer;
use crate::ast::{
    ComposeRoot, ComposeServiceOverride, ComposeTriggerOverride, NamedCompose, QueueRef, Span,
    Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawIdent, RawValue};

use super::compose::{ComposeSectionParts, COMPOSE_NO_KIND_HINT};

impl Analyzer {
    pub(super) fn lower_compose_compose(
        &mut self,
        root: &mut ComposeRoot,
        compose_names: &mut BTreeMap<String, Span>,
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
            self.compose_name_and_kind(kind, kind2, &keyword_span, "compose")
        else {
            return;
        };
        if let Some(extra) = kind_ident {
            self.errors.push(
                Diagnostic::error(
                    extra.span,
                    format!("unexpected kind `{}` after compose name", extra.name),
                )
                .with_hint(COMPOSE_NO_KIND_HINT),
            );
            return;
        }
        if let Some(prev) = compose_names.get(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate compose name `{}`", name_ident.name),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end
                )),
            );
            return;
        }
        compose_names.insert(name_ident.name.clone(), span.clone());
        if let Some(compose) = self.lower_compose_block(body, &name_ident) {
            root.composes.push(Spanned::new(compose, span));
        }
    }

    pub(super) fn lower_compose_block(
        &mut self,
        body: Option<RawBlock>,
        name_ident: &RawIdent,
    ) -> Option<NamedCompose> {
        let Some(block) = body else {
            self.errors.push(Diagnostic::error(
                name_ident.span.clone(),
                format!("compose `{}` requires a body", name_ident.name),
            ));
            return None;
        };

        let mut build_path: Option<(String, Span)> = None;
        let mut queue_overrides: BTreeMap<String, QueueRef> = BTreeMap::new();
        let mut service_overrides: BTreeMap<String, ComposeServiceOverride> = BTreeMap::new();
        let mut trigger_overrides: BTreeMap<String, ComposeTriggerOverride> = BTreeMap::new();

        for field in block.fields {
            match field.name.name.as_str() {
                "build" => match field.value {
                    RawValue::String(s, _) => {
                        build_path = Some((s, field.span));
                    }
                    other => {
                        self.errors.push(Diagnostic::error(
                            other.span(),
                            "compose `build` must be a string path",
                        ));
                    }
                },
                "queues" => self.lower_compose_queue_overrides(field.value, &mut queue_overrides),
                "services" => {
                    self.lower_compose_service_overrides(field.value, &mut service_overrides);
                }
                "triggers" => {
                    self.lower_compose_trigger_overrides(field.value, &mut trigger_overrides);
                }
                other => {
                    self.errors.push(Diagnostic::error(
                        field.span,
                        format!("unknown compose field `{other}`"),
                    ));
                }
            }
        }

        let Some((path, _)) = build_path else {
            self.errors.push(Diagnostic::error(
                name_ident.span.clone(),
                format!(
                    "compose `{}` requires a `build` field pointing to a compose.iter file",
                    name_ident.name
                ),
            ));
            return None;
        };

        Some(NamedCompose {
            name: name_ident.name.clone(),
            path: PathBuf::from(path),
            queues: queue_overrides,
            services: service_overrides,
            triggers: trigger_overrides,
        })
    }

    pub(super) fn lower_compose_queue_overrides(
        &mut self,
        value: RawValue,
        overrides: &mut BTreeMap<String, QueueRef>,
    ) {
        match value {
            RawValue::Block(b) => {
                for qf in b.fields {
                    let child_name = qf.name.name;
                    match qf.value {
                        RawValue::Ident(parent_name, _) => {
                            overrides.insert(child_name, QueueRef::Named(parent_name));
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                "queue override value must be a queue name (bareword)",
                            ));
                        }
                    }
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "compose `queues` must be a block `{ child = parent }`",
                ));
            }
        }
    }

    pub(super) fn lower_compose_service_overrides(
        &mut self,
        value: RawValue,
        overrides: &mut BTreeMap<String, ComposeServiceOverride>,
    ) {
        match value {
            RawValue::Block(b) => {
                for sf in b.fields {
                    let child_name = sf.name.name;
                    match sf.value {
                        RawValue::Block(sb) => {
                            let mut queue: Option<QueueRef> = None;
                            for attr in sb.fields {
                                if attr.name.name == "queue" {
                                    match attr.value {
                                        RawValue::Ident(n, _) => {
                                            queue = Some(QueueRef::Named(n));
                                        }
                                        other => {
                                            self.errors.push(Diagnostic::error(
                                                other.span(),
                                                "service override `queue` must be a queue name",
                                            ));
                                        }
                                    }
                                }
                            }
                            overrides.insert(child_name, ComposeServiceOverride { queue });
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                "service override must be a block `{ queue = ... }`",
                            ));
                        }
                    }
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "compose `services` must be a block",
                ));
            }
        }
    }

    pub(super) fn lower_compose_trigger_overrides(
        &mut self,
        value: RawValue,
        overrides: &mut BTreeMap<String, ComposeTriggerOverride>,
    ) {
        match value {
            RawValue::Block(b) => {
                for tf in b.fields {
                    let child_name = tf.name.name;
                    match tf.value {
                        RawValue::Ident(ref s, _) if s == "disabled" => {
                            overrides.insert(child_name, ComposeTriggerOverride::Disabled);
                        }
                        RawValue::Block(tb) => {
                            let mut target: Option<QueueRef> = None;
                            for attr in tb.fields {
                                if attr.name.name == "target" {
                                    match attr.value {
                                        RawValue::Ident(n, _) => {
                                            target = Some(QueueRef::Named(n));
                                        }
                                        other => {
                                            self.errors.push(Diagnostic::error(
                                                other.span(),
                                                "trigger override `target` must be a queue name",
                                            ));
                                        }
                                    }
                                }
                            }
                            overrides
                                .insert(child_name, ComposeTriggerOverride::Override { target });
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                "trigger override must be `disabled` or a block",
                            ));
                        }
                    }
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "compose `triggers` must be a block",
                ));
            }
        }
    }
}
