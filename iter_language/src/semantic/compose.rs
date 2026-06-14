//! `compose.iter` lowerer.
//!
//! Reuses the same CST/parser as the Iterfile path but interprets each
//! [`CstSection::Block`] differently: the first ident is the section *name*
//! (instead of the kind), and the optional second ident — when present — is
//! the kind. The compose file's grammar is otherwise identical to an
//! Iterfile body, so the per-kind builders (`lower_queue`, `lower_workspace`,
//! `lower_agent`, `lower_runner`, `lower_trigger`) are shared verbatim.

use std::collections::BTreeMap;

use super::Analyzer;
use super::compose_resolve::resolve_queue_refs;
use crate::ast::{Compose, NamedQueue, Span, Spanned, TelemetryDef, TelemetryProtocol};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstFile, CstIdent, CstSection, CstValue};

const QUEUE_REQUIRES_KIND_HINT: &str = "compose.iter queues take a name *and* a backend kind: e.g. `queue main file { path = \"./.iter/queue\" }`.";
pub(super) const TRIGGER_REQUIRES_KIND_HINT: &str = "compose.iter triggers take a name *and* a kind: e.g. `trigger nightly cron { schedule = \"0 0 * * *\" target = main }`.";
pub(super) const SERVICE_NO_KIND_HINT: &str = "compose.iter services take a name only: `service runner { build = \"./Iterfile\" queue = main }`.";
pub(super) const COMPOSE_NO_KIND_HINT: &str = "compose.iter compose blocks take a name only: `compose child { build = \"./child/compose.iter\" }`.";
const TELEMETRY_NO_KIND_HINT: &str = "compose.iter telemetry is a singleton block: `telemetry { endpoint = \"http://collector:4318\" }`.";

pub(crate) fn lower_compose_and_check(file: CstFile) -> (Option<Compose>, Vec<Diagnostic>) {
    let mut analyzer = Analyzer::default();
    let result = analyzer.lower_compose(file);
    // Note: `finish_arg_refs` is deliberately Iterfile-only. A compose.iter
    // has no `arg` *declaration* surface — its only args are concrete
    // build-time values a service passes down to its Iterfile — so there is
    // no declared-name set to cross-check against here. Any `{{arg.*}}`
    // recorded while lowering a compose file is therefore intentionally
    // dropped rather than reported. Compose-side arg legality belongs to the
    // operator layer that resolves those values.
    (Some(result), analyzer.errors)
}

#[derive(Default)]
struct ComposeNameSets {
    queues: BTreeMap<String, Span>,
    services: BTreeMap<String, Span>,
    triggers: BTreeMap<String, Span>,
    composes: BTreeMap<String, Span>,
    telemetry: Option<Span>,
}

pub(super) struct ComposeSectionParts {
    pub(super) kind: Option<CstIdent>,
    pub(super) kind2: Option<CstIdent>,
    pub(super) body: Option<CstBlock>,
    pub(super) keyword_span: Span,
    pub(super) span: Span,
}

impl Analyzer {
    fn lower_compose(&mut self, file: CstFile) -> Compose {
        let mut root = Compose::default();
        let mut names = ComposeNameSets::default();

        for section in file.sections {
            match section {
                CstSection::Block {
                    keyword,
                    keyword_span,
                    kind,
                    kind2,
                    alias,
                    body,
                    span,
                } => {
                    self.lower_compose_block_section(
                        &mut root,
                        &mut names,
                        &keyword,
                        alias.as_ref(),
                        ComposeSectionParts {
                            kind,
                            kind2,
                            body,
                            keyword_span,
                            span,
                        },
                    );
                }
                CstSection::Prompt { span, .. } => {
                    self.errors.push(
                        Diagnostic::error(
                            span,
                            "`prompt` is not a valid top-level compose.iter section",
                        )
                        .with_hint(
                            "prompt declarations belong inside a `service` block (inline form).",
                        ),
                    );
                }
                CstSection::On { span, .. } => {
                    self.errors.push(
                        Diagnostic::error(
                            span,
                            "`on` event handlers are not valid at compose.iter top level",
                        )
                        .with_hint("event handlers belong inside a `service` block (inline form)."),
                    );
                }
            }
        }

        if let Err(diag) = resolve_queue_refs(&mut root) {
            self.errors.push(diag);
        }

        root
    }

    fn lower_compose_block_section(
        &mut self,
        root: &mut Compose,
        names: &mut ComposeNameSets,
        keyword: &str,
        alias: Option<&CstIdent>,
        parts: ComposeSectionParts,
    ) {
        if let Some(a) = alias {
            self.errors.push(Diagnostic::error(
                a.span.clone(),
                format!("`as {}` naming is not valid in compose.iter", a.name),
            ).with_hint("compose.iter uses `<keyword> <name> [<kind>] {{ ... }}` — the first identifier is the name."));
        }
        match keyword {
            "queue" => self.lower_compose_queue(root, &mut names.queues, parts),
            "service" => self.lower_compose_service_section(root, &mut names.services, parts),
            "trigger" => self.lower_compose_trigger(root, &mut names.triggers, parts),
            "compose" => self.lower_compose_compose(root, &mut names.composes, parts),
            "telemetry" => self.lower_compose_telemetry(root, &mut names.telemetry, parts),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        parts.keyword_span,
                        format!("unknown compose.iter top-level keyword `{other}`"),
                    )
                    .with_hint("expected one of: queue, service, trigger, compose, telemetry."),
                );
            }
        }
    }

    fn lower_compose_queue(
        &mut self,
        root: &mut Compose,
        queue_names: &mut BTreeMap<String, Span>,
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
            self.compose_name_and_kind(kind, kind2, &keyword_span, "queue")
        else {
            return;
        };
        let Some(kind_ident) = kind_ident else {
            self.errors.push(
                Diagnostic::error(
                    name_ident.span,
                    "compose.iter `queue` requires a backend kind",
                )
                .with_hint(QUEUE_REQUIRES_KIND_HINT),
            );
            return;
        };
        if let Some(prev) = queue_names.get(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate queue name `{}`", name_ident.name),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end
                )),
            );
            return;
        }
        queue_names.insert(name_ident.name.clone(), span.clone());
        if let Some(decl) = self.lower_queue(Some(kind_ident), body, &keyword_span) {
            root.queues.push(Spanned::new(
                NamedQueue {
                    name: name_ident.name,
                    decl,
                },
                span,
            ));
        }
    }

    fn lower_compose_telemetry(
        &mut self,
        root: &mut Compose,
        telemetry_seen: &mut Option<Span>,
        parts: ComposeSectionParts,
    ) {
        let ComposeSectionParts {
            kind,
            kind2,
            body,
            keyword_span,
            span,
        } = parts;
        if let Some(extra) = kind.or(kind2) {
            self.errors.push(
                Diagnostic::error(
                    extra.span,
                    format!("unexpected kind `{}` after telemetry", extra.name),
                )
                .with_hint(TELEMETRY_NO_KIND_HINT),
            );
            return;
        }
        if let Some(prev) = telemetry_seen {
            self.errors.push(
                Diagnostic::error(span.clone(), "duplicate telemetry block").with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end
                )),
            );
            return;
        }
        *telemetry_seen = Some(span.clone());
        let decl = self.lower_telemetry(body, &keyword_span);
        root.telemetry = Some(Spanned::new(decl, span));
    }

    fn lower_telemetry(&mut self, body: Option<CstBlock>, keyword_span: &Span) -> TelemetryDef {
        let mut fields = self.collect_fields(body);
        let enabled = self
            .take_optional_bool(&mut fields, "enabled")
            .unwrap_or(true);
        let service_name = self.take_optional_string(&mut fields, "service_name");
        let service_namespace = self.take_optional_string(&mut fields, "service_namespace");
        let endpoint = self.take_optional_string(&mut fields, "endpoint");
        let protocol = match self
            .take_optional_string(&mut fields, "protocol")
            .as_deref()
        {
            None | Some("http/protobuf") => TelemetryProtocol::HttpProtobuf,
            Some(other) => {
                self.errors.push(
                    Diagnostic::error(
                        keyword_span.clone(),
                        format!("unsupported telemetry protocol `{other}`"),
                    )
                    .with_hint("supported protocol: `http/protobuf`"),
                );
                TelemetryProtocol::HttpProtobuf
            }
        };
        let resource_attributes = self.take_optional_string_map(&mut fields, "resource_attributes");
        self.reject_unknown_fields(
            &mut fields,
            &[
                "enabled",
                "service_name",
                "service_namespace",
                "endpoint",
                "protocol",
                "resource_attributes",
            ],
            "telemetry block",
        );
        TelemetryDef {
            enabled,
            service_name,
            service_namespace,
            endpoint,
            protocol,
            resource_attributes: resource_attributes.unwrap_or_default(),
        }
    }

    fn take_optional_string_map(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<BTreeMap<String, String>> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Block(block) => {
                let mut out = BTreeMap::new();
                for attr in block.fields {
                    match attr.value {
                        CstValue::String(value, _) => {
                            out.insert(attr.name.name, value);
                        }
                        other @ (CstValue::Integer(..)
                        | CstValue::Duration(..)
                        | CstValue::Bool(..)
                        | CstValue::Null(_)
                        | CstValue::Ident(..)
                        | CstValue::List(..)
                        | CstValue::Block(_)
                        | CstValue::Call { .. }) => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                format!("`{name}` values must be strings"),
                            ));
                        }
                    }
                }
                Some(out)
            }
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Ident(..)
            | CstValue::List(..)
            | CstValue::Call { .. }) => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a block of string values"),
                ));
                None
            }
        }
    }

    pub(super) fn compose_name_and_kind(
        &mut self,
        kind: Option<CstIdent>,
        kind2: Option<CstIdent>,
        keyword_span: &Span,
        keyword: &str,
    ) -> Option<(CstIdent, Option<CstIdent>)> {
        let Some(name) = kind else {
            self.errors.push(Diagnostic::error(
                keyword_span.clone(),
                format!("compose.iter `{keyword}` requires a name"),
            ));
            return None;
        };
        Some((name, kind2))
    }
}
