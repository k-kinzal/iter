use std::collections::BTreeMap;
use std::path::PathBuf;

use super::Analyzer;
use crate::ast::{
    InlineService, NamedService, QueueRef, RunnerDecl, ServiceSource, Span, Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawIdent, RawValue};

use super::compose::{ComposeSectionParts, SERVICE_NO_KIND_HINT};

use crate::ast::ComposeRoot;

impl Analyzer {
    pub(super) fn lower_compose_service_section(
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

    pub(super) fn lower_compose_service(
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

    pub(super) fn lower_inline_service_field(
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
}
