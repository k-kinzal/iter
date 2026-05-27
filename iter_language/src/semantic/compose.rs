//! `compose.iter` lowerer.
//!
//! Reuses the same CST/parser as the Iterfile path but interprets each
//! [`RawSection::Block`] differently: the first ident is the section *name*
//! (instead of the kind), and the optional second ident — when present — is
//! the kind. The compose file's grammar is otherwise identical to an
//! Iterfile body, so the per-kind builders (`lower_queue`, `lower_workspace`,
//! `lower_agent`, `lower_runner`, `lower_trigger`) are shared verbatim.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::Analyzer;
use crate::ast::{
    ComposeRoot, ComposeServiceOverride, ComposeTriggerOverride, InlineService, NamedCompose,
    NamedQueue, NamedService, NamedTrigger, QueueRef, RunnerDecl, ServiceSource, Span, Spanned,
    TelemetryDecl, TelemetryProtocol, TriggerDecl,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawFile, RawIdent, RawSection, RawValue};

const QUEUE_REQUIRES_KIND_HINT: &str = "compose.iter queues take a name *and* a backend kind: e.g. `queue main file { path = \"./.iter/queue\" }`.";
const TRIGGER_REQUIRES_KIND_HINT: &str = "compose.iter triggers take a name *and* a kind: e.g. `trigger nightly cron { schedule = \"0 0 * * *\" target = main }`.";
const SERVICE_NO_KIND_HINT: &str = "compose.iter services take a name only: `service runner { build = \"./Iterfile\" queue = main }`.";
const COMPOSE_NO_KIND_HINT: &str = "compose.iter compose blocks take a name only: `compose child { build = \"./child/compose.iter\" }`.";
const TELEMETRY_NO_KIND_HINT: &str = "compose.iter telemetry is a singleton block: `telemetry { endpoint = \"http://collector:4318\" }`.";

pub(crate) fn lower_compose_and_check(file: RawFile) -> (Option<ComposeRoot>, Vec<Diagnostic>) {
    let mut analyzer = Analyzer::default();
    let result = analyzer.lower_compose(file);
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

struct ComposeSectionParts {
    kind: Option<RawIdent>,
    kind2: Option<RawIdent>,
    body: Option<RawBlock>,
    keyword_span: Span,
    span: Span,
}

impl Analyzer {
    #[allow(clippy::too_many_lines)]
    fn lower_compose(&mut self, file: RawFile) -> ComposeRoot {
        let mut root = ComposeRoot::default();
        let mut names = ComposeNameSets::default();

        for section in file.sections {
            match section {
                RawSection::Block {
                    keyword,
                    keyword_span,
                    kind,
                    kind2,
                    alias,
                    body,
                    span,
                } => {
                    if let Some(ref a) = alias {
                        self.errors.push(Diagnostic::error(
                            a.span.clone(),
                            format!("`as {}` naming is not valid in compose.iter", a.name),
                        ).with_hint("compose.iter uses `<keyword> <name> [<kind>] {{ ... }}` — the first identifier is the name."));
                    }
                    match keyword.as_str() {
                    "queue" => {
                        self.lower_compose_queue(
                            &mut root,
                            &mut names.queues,
                            ComposeSectionParts {
                                kind,
                                kind2,
                                body,
                                keyword_span,
                                span,
                            },
                        );
                    }
                    "service" => {
                        self.lower_compose_service_section(
                            &mut root,
                            &mut names.services,
                            ComposeSectionParts {
                                kind,
                                kind2,
                                body,
                                keyword_span,
                                span,
                            },
                        );
                    }
                    "trigger" => {
                        self.lower_compose_trigger(
                            &mut root,
                            &mut names.triggers,
                            ComposeSectionParts {
                                kind,
                                kind2,
                                body,
                                keyword_span,
                                span,
                            },
                        );
                    }
                    "compose" => {
                        self.lower_compose_compose(
                            &mut root,
                            &mut names.composes,
                            ComposeSectionParts {
                                kind,
                                kind2,
                                body,
                                keyword_span,
                                span,
                            },
                        );
                    }
                    "telemetry" => {
                        self.lower_compose_telemetry(
                            &mut root,
                            &mut names.telemetry,
                            ComposeSectionParts {
                                kind,
                                kind2,
                                body,
                                keyword_span,
                                span,
                            },
                        );
                    }
                    other => {
                        self.errors.push(
                            Diagnostic::error(
                                keyword_span,
                                format!("unknown compose.iter top-level keyword `{other}`"),
                            )
                            .with_hint(
                                "expected one of: queue, service, trigger, compose, telemetry.",
                            ),
                        );
                    }
                }
                }
                RawSection::Prompt { span, .. } => {
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
                RawSection::On { span, .. } => {
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

    fn lower_compose_queue(
        &mut self,
        root: &mut ComposeRoot,
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

    fn lower_compose_service_section(
        &mut self,
        root: &mut ComposeRoot,
        service_names: &mut BTreeMap<String, Span>,
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
            self.compose_name_and_kind(kind, kind2, &keyword_span, "service")
        else {
            return;
        };
        if let Some(extra) = kind_ident {
            self.errors.push(
                Diagnostic::error(
                    extra.span,
                    format!("unexpected kind `{}` after service name", extra.name),
                )
                .with_hint(SERVICE_NO_KIND_HINT),
            );
            return;
        }
        if let Some(prev) = service_names.get(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate service name `{}`", name_ident.name),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end
                )),
            );
            return;
        }
        service_names.insert(name_ident.name.clone(), span.clone());
        if let Some(source) = self.lower_compose_service(body, &name_ident) {
            root.services.push(Spanned::new(
                NamedService {
                    name: name_ident.name,
                    source,
                },
                span,
            ));
        }
    }

    fn lower_compose_trigger(
        &mut self,
        root: &mut ComposeRoot,
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

    fn lower_compose_compose(
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

    fn lower_compose_telemetry(
        &mut self,
        root: &mut ComposeRoot,
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

    fn lower_telemetry(&mut self, body: Option<RawBlock>, keyword_span: &Span) -> TelemetryDecl {
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
        TelemetryDecl {
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
        fields: &mut BTreeMap<String, RawField>,
        name: &str,
    ) -> Option<BTreeMap<String, String>> {
        let field = fields.remove(name)?;
        match field.value {
            RawValue::Block(block) => {
                let mut out = BTreeMap::new();
                for attr in block.fields {
                    match attr.value {
                        RawValue::String(value, _) => {
                            out.insert(attr.name.name, value);
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                format!("`{name}` values must be strings"),
                            ));
                        }
                    }
                }
                Some(out)
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a block of string values"),
                ));
                None
            }
        }
    }

    fn lower_compose_block(
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

    fn lower_compose_queue_overrides(
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

    fn lower_compose_service_overrides(
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

    fn lower_compose_trigger_overrides(
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

    fn compose_name_and_kind(
        &mut self,
        kind: Option<RawIdent>,
        kind2: Option<RawIdent>,
        keyword_span: &Span,
        keyword: &str,
    ) -> Option<(RawIdent, Option<RawIdent>)> {
        let Some(name) = kind else {
            self.errors.push(Diagnostic::error(
                keyword_span.clone(),
                format!("compose.iter `{keyword}` requires a name"),
            ));
            return None;
        };
        Some((name, kind2))
    }

    fn lower_compose_service(
        &mut self,
        body: Option<RawBlock>,
        name_ident: &RawIdent,
    ) -> Option<ServiceSource> {
        let Some(block) = body else {
            self.errors.push(Diagnostic::error(
                name_ident.span.clone(),
                format!("service `{}` requires a body", name_ident.name),
            ));
            return None;
        };

        let mut build_path: Option<(String, Span)> = None;
        let mut queue_ref: Option<QueueRef> = None;
        let mut arg_overrides: BTreeMap<String, String> = BTreeMap::new();
        let mut workspace_section: Option<Spanned<crate::ast::WorkspaceDecl>> = None;
        let mut agent_section: Option<Spanned<crate::ast::AgentDecl>> = None;
        let mut runner_section: Option<Spanned<RunnerDecl>> = None;
        let mut prompts: Vec<Spanned<crate::ast::PromptDecl>> = Vec::new();
        let mut events: Vec<Spanned<crate::ast::EventHandlerDecl>> = Vec::new();
        let mut leftover_fields: Vec<RawField> = Vec::new();

        for field in block.fields {
            match field.name.name.as_str() {
                "build" => match field.value {
                    RawValue::String(s, span) => {
                        build_path = Some((s, span));
                    }
                    other => {
                        self.errors.push(Diagnostic::error(
                            other.span(),
                            "service `build` must be a string path",
                        ));
                    }
                },
                "queue" => match field.value {
                    RawValue::Ident(name, _) => {
                        queue_ref = Some(QueueRef::Named(name));
                    }
                    other => {
                        self.errors.push(Diagnostic::error(
                            other.span(),
                            "service `queue` must reference a queue name (bareword)",
                        ));
                    }
                },
                "args" => match field.value {
                    RawValue::Block(args_block) => {
                        for args_field in args_block.fields {
                            match args_field.value {
                                RawValue::String(s, _) => {
                                    arg_overrides.insert(args_field.name.name.clone(), s);
                                }
                                other => {
                                    self.errors.push(Diagnostic::error(
                                        other.span(),
                                        "service `args` values must be strings",
                                    ));
                                }
                            }
                        }
                    }
                    other => {
                        self.errors.push(Diagnostic::error(
                            other.span(),
                            "service `args` must be a block of key = \"value\" pairs",
                        ));
                    }
                },
                _ => leftover_fields.push(field),
            }
        }

        for field in std::mem::take(&mut leftover_fields) {
            self.lower_inline_service_field(
                field,
                &mut workspace_section,
                &mut agent_section,
                &mut runner_section,
            );
        }
        let _ = (&mut prompts, &mut events);

        if let Some((path, _)) = build_path {
            Some(ServiceSource::Build {
                path: PathBuf::from(path),
                queue: queue_ref,
                args: arg_overrides,
            })
        } else {
            Some(ServiceSource::Inline(Box::new(InlineService {
                queue: queue_ref,
                workspace: workspace_section,
                agent: agent_section,
                runner: runner_section,
                prompts,
                events,
            })))
        }
    }

    fn lower_inline_service_field(
        &mut self,
        field: RawField,
        workspace_section: &mut Option<Spanned<crate::ast::WorkspaceDecl>>,
        agent_section: &mut Option<Spanned<crate::ast::AgentDecl>>,
        runner_section: &mut Option<Spanned<RunnerDecl>>,
    ) {
        let RawValue::Block(_) = field.value else {
            self.errors.push(Diagnostic::error(
                field.span,
                format!("unexpected field `{}` in service body", field.name.name),
            ));
            return;
        };
        let sub_keyword = field.name.name.clone();
        let sub_keyword_span = field.name.span.clone();
        let sub_body = match field.value {
            RawValue::Block(b) => Some(b),
            _ => unreachable!(),
        };
        match sub_keyword.as_str() {
            "workspace_clone" | "workspace_local" | "workspace_sandbox" => {
                let kind_str = match sub_keyword.as_str() {
                    "workspace_clone" => "clone",
                    "workspace_local" => "local",
                    "workspace_sandbox" => "sandbox",
                    _ => unreachable!(),
                };
                let kind_ident = RawIdent {
                    name: kind_str.to_string(),
                    span: sub_keyword_span.clone(),
                };
                if let Some(decl) =
                    self.lower_workspace(Some(kind_ident), sub_body, &sub_keyword_span)
                {
                    *workspace_section = Some(Spanned::new(decl, sub_keyword_span));
                }
            }
            "agent_claude" | "agent_codex" | "agent_gemini" | "agent_hermes"
            | "agent_antigravity" | "agent_copilot" | "agent_cursor" | "agent_cline"
            | "agent_opencode" | "agent_generic" => {
                let kind_str = sub_keyword.strip_prefix("agent_").unwrap();
                let kind_ident = RawIdent {
                    name: kind_str.to_string(),
                    span: sub_keyword_span.clone(),
                };
                if let Some(decl) = self.lower_agent(Some(kind_ident), sub_body, &sub_keyword_span)
                {
                    *agent_section = Some(Spanned::new(decl, sub_keyword_span));
                }
            }
            "runner" => {
                if let Some(decl) = self.lower_runner_old(None, sub_body, &sub_keyword_span) {
                    *runner_section = Some(Spanned::new(
                        RunnerDecl {
                            name: None,
                            agent: String::new(),
                            workspace: String::new(),
                            queue: None,
                            continue_on_error: decl.continue_on_error,
                            behavior: decl.behavior,
                            iteration_timeout_secs: decl.iteration_timeout_secs,
                            prompt: crate::ast::PromptExpr::Single(
                                crate::ast::PromptValue::Inline(String::new()),
                            ),
                            events: Vec::new(),
                        },
                        sub_keyword_span,
                    ));
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    sub_keyword_span,
                    format!("unknown nested service section `{other}`"),
                ));
            }
        }
    }

    fn lower_trigger_with_target(
        &mut self,
        kind: RawIdent,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<TriggerDecl> {
        // Strip compose-only fields before delegating to the shared
        // trigger lowerer so it does not emit "unknown field" diagnostics.
        let body = body.map(|b| RawBlock {
            fields: b
                .fields
                .into_iter()
                .filter(|f| f.name.name != "target" && f.name.name != "terminate_on_completion")
                .collect(),
            routes: b.routes,
            actions: b.actions,
            span: b.span,
        });
        self.lower_trigger(Some(kind), body, keyword_span)
    }
}

fn take_target_field(body: Option<&RawBlock>, errors: &mut Vec<Diagnostic>) -> QueueRef {
    let Some(block) = body else {
        return QueueRef::Anonymous;
    };
    for field in &block.fields {
        if field.name.name == "target" {
            match &field.value {
                RawValue::Ident(name, _) => return QueueRef::Named(name.clone()),
                other => {
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

fn take_bool_field(body: Option<&RawBlock>, name: &str, errors: &mut Vec<Diagnostic>) -> bool {
    let Some(block) = body else {
        return false;
    };
    for field in &block.fields {
        if field.name.name == name {
            match &field.value {
                RawValue::Bool(val, _) => return *val,
                other => {
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

fn resolve_queue_refs(root: &mut ComposeRoot) -> Result<(), Diagnostic> {
    let queue_count = root.queues.len();
    let single_queue_name = if queue_count == 1 {
        Some(root.queues[0].node.name.clone())
    } else {
        None
    };

    for service in &mut root.services {
        let queue_slot: &mut Option<QueueRef> = match &mut service.node.source {
            ServiceSource::Build { queue, .. } => queue,
            ServiceSource::Inline(inline) => &mut inline.queue,
        };
        check_or_default(
            queue_slot,
            single_queue_name.as_deref(),
            queue_count,
            &service.span,
        )?;
        validate_named(queue_slot.as_ref(), &root.queues, &service.span)?;
    }

    for trigger in &mut root.triggers {
        let mut slot = Some(std::mem::replace(
            &mut trigger.node.target,
            QueueRef::Anonymous,
        ));
        check_or_default(
            &mut slot,
            single_queue_name.as_deref(),
            queue_count,
            &trigger.span,
        )?;
        validate_named(slot.as_ref(), &root.queues, &trigger.span)?;
        trigger.node.target = slot.unwrap_or(QueueRef::Anonymous);
    }

    for compose in &root.composes {
        for queue_ref in compose.node.queues.values() {
            validate_named(Some(queue_ref), &root.queues, &compose.span)?;
        }
        for svc_override in compose.node.services.values() {
            if let Some(ref queue_ref) = svc_override.queue {
                validate_named(Some(queue_ref), &root.queues, &compose.span)?;
            }
        }
        for trig_override in compose.node.triggers.values() {
            if let ComposeTriggerOverride::Override {
                target: Some(queue_ref),
            } = trig_override
            {
                validate_named(Some(queue_ref), &root.queues, &compose.span)?;
            }
        }
    }

    Ok(())
}

fn check_or_default(
    slot: &mut Option<QueueRef>,
    single_queue_name: Option<&str>,
    queue_count: usize,
    span: &Span,
) -> Result<(), Diagnostic> {
    let needs_default = matches!(slot, None | Some(QueueRef::Anonymous));
    if needs_default {
        if let Some(name) = single_queue_name {
            *slot = Some(QueueRef::Named(name.to_owned()));
            return Ok(());
        }
        return Err(Diagnostic::error(
            span.clone(),
            if queue_count == 0 {
                "compose.iter declares no `queue` blocks; add one or qualify the binding"
            } else {
                "queue reference omitted but compose.iter declares more than one queue"
            },
        ));
    }
    Ok(())
}

fn validate_named(
    slot: Option<&QueueRef>,
    queues: &[Spanned<NamedQueue>],
    span: &Span,
) -> Result<(), Diagnostic> {
    if let Some(QueueRef::Named(name)) = slot {
        if !queues.iter().any(|q| &q.node.name == name) {
            return Err(Diagnostic::error(
                span.clone(),
                format!("queue `{name}` is not declared in this compose.iter"),
            ));
        }
    }
    Ok(())
}
