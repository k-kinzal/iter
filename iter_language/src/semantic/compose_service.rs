use std::collections::BTreeMap;
use std::path::PathBuf;

use super::Analyzer;
use crate::ast::{InlineService, NamedService, QueueRef, RunnerDef, ServiceSource, Span, Spanned};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstIdent, CstValue};

use super::compose::{ComposeSectionParts, SERVICE_NO_KIND_HINT};

use crate::ast::Compose;

impl Analyzer {
    pub(super) fn lower_compose_service_section(
        &mut self,
        root: &mut Compose,
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

    pub(super) fn lower_compose_service(
        &mut self,
        body: Option<CstBlock>,
        name_ident: &CstIdent,
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
        let mut workspace_section: Option<Spanned<crate::ast::WorkspaceDef>> = None;
        let mut agent_section: Option<Spanned<crate::ast::AgentDef>> = None;
        let mut runner_section: Option<Spanned<RunnerDef>> = None;
        let mut leftover_fields: Vec<CstField> = Vec::new();

        for field in block.fields {
            match field.name.name.as_str() {
                "build" => match field.value {
                    CstValue::String(s, span) => {
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
                    CstValue::Ident(name, _) => {
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
                    CstValue::Block(args_block) => {
                        for args_field in args_block.fields {
                            match args_field.value {
                                CstValue::String(s, _) => {
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
            })))
        }
    }

    pub(super) fn lower_inline_service_field(
        &mut self,
        field: CstField,
        workspace_section: &mut Option<Spanned<crate::ast::WorkspaceDef>>,
        agent_section: &mut Option<Spanned<crate::ast::AgentDef>>,
        runner_section: &mut Option<Spanned<RunnerDef>>,
    ) {
        let CstValue::Block(_) = field.value else {
            self.errors.push(Diagnostic::error(
                field.span,
                format!("unexpected field `{}` in service body", field.name.name),
            ));
            return;
        };
        let sub_keyword = field.name.name.clone();
        let sub_keyword_span = field.name.span.clone();
        let sub_body = match field.value {
            CstValue::Block(b) => Some(b),
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
                let kind_ident = CstIdent {
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
            | "agent_opencode" | "agent_grok" | "agent_generic" => {
                let kind_str = sub_keyword.strip_prefix("agent_").unwrap();
                let kind_ident = CstIdent {
                    name: kind_str.to_string(),
                    span: sub_keyword_span.clone(),
                };
                if let Some(decl) = self.lower_agent(Some(kind_ident), sub_body, &sub_keyword_span)
                {
                    *agent_section = Some(Spanned::new(decl, sub_keyword_span));
                }
            }
            "runner" => {
                if let Some(decl) = self.lower_runner_inline(sub_body, &sub_keyword_span) {
                    *runner_section = Some(Spanned::new(decl, sub_keyword_span));
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
}
