//! `agent { ... }` lowerer plus mode/apply-back identifier parsers shared with field helpers.

use super::{Analyzer, COMMAND_HINT};
use crate::ast::{AgentDecl, AgentMode, CloneApplyBackMode, RouterStrategy, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawIdent, RawValue};

struct SimpleAgentParts {
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
}

impl Analyzer {
    pub(super) fn lower_agent(
        &mut self,
        kind: Option<RawIdent>,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<AgentDecl> {
        let kind = self.require_kind(
            kind,
            keyword_span,
            "agent",
            &[
                "claude",
                "codex",
                "gemini",
                "antigravity",
                "copilot",
                "cursor",
                "cline",
                "opencode",
                "generic",
                "router",
            ],
        )?;
        if kind.name == "router" {
            return self.lower_router_agent(&kind, body);
        }
        let mut fields = self.collect_fields(body);
        let decl = match kind.name.as_str() {
            "claude" | "codex" | "gemini" | "antigravity" | "copilot" => {
                self.lower_mode_agent(&kind, &mut fields)?
            }
            "cursor" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "cursor")?;
                AgentDecl::Cursor { command, args, env }
            }
            "cline" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "cline")?;
                AgentDecl::Cline { command, args, env }
            }
            "opencode" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "opencode")?;
                AgentDecl::OpenCode { command, args, env }
            }
            "generic" => self.lower_generic_agent(&kind, &mut fields),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind.span,
                        format!("unknown agent kind `{other}`"),
                    )
                    .with_hint(
                        "valid kinds: claude, codex, gemini, antigravity, copilot, cursor, cline, opencode, generic, router",
                    ),
                );
                return None;
            }
        };
        Some(decl)
    }

    fn lower_mode_agent(
        &mut self,
        kind: &RawIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::RawField>,
    ) -> Option<AgentDecl> {
        let mode = self.take_required_agent_mode(fields, &kind.span, &kind.name);
        let command = self.take_required_string_with_hint(
            fields,
            "command",
            &kind.span,
            &format!("agent {}", kind.name),
            COMMAND_HINT,
        );
        let args = self
            .take_optional_string_list(fields, "args")
            .unwrap_or_default();
        let env = self.take_optional_env_block(fields);
        match kind.name.as_str() {
            "claude" => {
                let session_id_file = self.take_optional_string(fields, "session_id_file");
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "session_id_file", "env"],
                    "agent claude",
                );
                Some(AgentDecl::Claude {
                    mode: mode?,
                    command: command?,
                    args,
                    session_id_file,
                    env,
                })
            }
            "codex" => {
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "env"],
                    "agent codex",
                );
                Some(AgentDecl::Codex {
                    mode: mode?,
                    command: command?,
                    args,
                    env,
                })
            }
            "gemini" => {
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "env"],
                    "agent gemini",
                );
                Some(AgentDecl::Gemini {
                    mode: mode?,
                    command: command?,
                    args,
                    env,
                })
            }
            "antigravity" => {
                let conversation_id = self.take_optional_string(fields, "conversation_id");
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "conversation_id", "env"],
                    "agent antigravity",
                );
                Some(AgentDecl::Antigravity {
                    mode: mode?,
                    command: command?,
                    args,
                    conversation_id,
                    env,
                })
            }
            "copilot" => {
                let subcommand = self.take_optional_string_list(fields, "subcommand");
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "subcommand", "args", "env"],
                    "agent copilot",
                );
                Some(AgentDecl::Copilot {
                    mode: mode?,
                    command: command?,
                    subcommand,
                    args,
                    env,
                })
            }
            _ => unreachable!(),
        }
    }

    fn lower_simple_agent(
        &mut self,
        kind: &RawIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::RawField>,
        label: &str,
    ) -> Option<SimpleAgentParts> {
        let command = self.take_required_string_with_hint(
            fields,
            "command",
            &kind.span,
            &format!("agent {label}"),
            COMMAND_HINT,
        );
        let args = self
            .take_optional_string_list(fields, "args")
            .unwrap_or_default();
        let env = self.take_optional_env_block(fields);
        self.reject_unknown_fields(
            fields,
            &["command", "args", "env"],
            &format!("agent {label}"),
        );
        Some(SimpleAgentParts {
            command: command?,
            args,
            env,
        })
    }

    fn lower_generic_agent(
        &mut self,
        kind: &RawIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::RawField>,
    ) -> AgentDecl {
        let command = if let Some(field) = fields.remove("command") {
            match field.value {
                RawValue::List(items, _) => {
                    let mut out = Vec::new();
                    for item in items {
                        match item {
                            RawValue::String(s, _) => out.push(s),
                            other => {
                                self.errors.push(Diagnostic::error(
                                    other.span(),
                                    "`command` list elements must be strings",
                                ));
                            }
                        }
                    }
                    out
                }
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`command` must be a list of strings",
                    ));
                    Vec::new()
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(kind.span.clone(), "agent generic requires `command`")
                    .with_hint("add `command = [\"prog\", \"--flag\"]`"),
            );
            Vec::new()
        };
        let env = self.take_optional_env_block(fields);
        self.reject_unknown_fields(fields, &["command", "env"], "agent generic");
        AgentDecl::Generic { command, env }
    }

    fn lower_router_agent(&mut self, kind: &RawIdent, body: Option<RawBlock>) -> Option<AgentDecl> {
        let raw_fields = match body {
            Some(block) => block.fields,
            None => Vec::new(),
        };

        let mut strategy = RouterStrategy::Fallback;
        let mut agents: Vec<(String, Box<AgentDecl>)> = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for field in raw_fields {
            let name = field.name.name.clone();

            if name == "strategy" {
                if !seen_names.insert(name) {
                    self.errors.push(Diagnostic::error(
                        field.name.span.clone(),
                        "duplicate field `strategy` in block",
                    ));
                    continue;
                }
                match field.value {
                    RawValue::Ident(ref ident, ref span) => match ident.as_str() {
                        "fallback" => strategy = RouterStrategy::Fallback,
                        "rotate" => strategy = RouterStrategy::Rotate,
                        other => {
                            self.errors.push(
                                Diagnostic::error(
                                    span.clone(),
                                    format!("unknown router strategy `{other}`"),
                                )
                                .with_hint("valid strategies: fallback, rotate"),
                            );
                            return None;
                        }
                    },
                    other => {
                        self.errors.push(Diagnostic::error(
                            other.span(),
                            "`strategy` must be an identifier (fallback or rotate)",
                        ));
                        return None;
                    }
                }
                continue;
            }

            if !seen_names.insert(name.clone()) {
                self.errors.push(Diagnostic::error(
                    field.name.span.clone(),
                    format!("duplicate field `{name}` in block"),
                ));
                continue;
            }

            match field.value {
                RawValue::Block(block) => {
                    let mut sub_fields = self.collect_fields(Some(block));
                    let sub_kind = if let Some(kind_field) = sub_fields.remove("kind") {
                        match kind_field.value {
                            RawValue::Ident(s, _) => s,
                            other => {
                                self.errors.push(Diagnostic::error(
                                    other.span(),
                                    "`kind` must be an identifier",
                                ));
                                continue;
                            }
                        }
                    } else {
                        self.errors.push(
                            Diagnostic::error(
                                field.name.span.clone(),
                                format!("router sub-agent `{name}` requires `kind`"),
                            )
                            .with_hint("add `kind = claude` (or codex, gemini, etc.)"),
                        );
                        continue;
                    };
                    let sub_ident = RawIdent {
                        name: sub_kind,
                        span: field.name.span.clone(),
                    };
                    let sub_decl = self.lower_sub_agent(&sub_ident, &mut sub_fields);
                    if let Some(decl) = sub_decl {
                        agents.push((name, Box::new(decl)));
                    }
                }
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        format!(
                            "router sub-agent `{name}` must be a block (`{name} {{ kind = ... }}`)"
                        ),
                    ));
                }
            }
        }

        if agents.is_empty() {
            self.errors.push(
                Diagnostic::error(
                    kind.span.clone(),
                    "agent router requires at least one sub-agent",
                )
                .with_hint("add named sub-agent blocks: `primary { kind = claude; ... }`"),
            );
            return None;
        }

        Some(AgentDecl::Router { agents, strategy })
    }

    fn lower_sub_agent(
        &mut self,
        kind: &RawIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::RawField>,
    ) -> Option<AgentDecl> {
        match kind.name.as_str() {
            "claude" | "codex" | "gemini" | "antigravity" | "copilot" => {
                self.lower_mode_agent(kind, fields)
            }
            "cursor" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "cursor")?;
                Some(AgentDecl::Cursor { command, args, env })
            }
            "cline" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "cline")?;
                Some(AgentDecl::Cline { command, args, env })
            }
            "opencode" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "opencode")?;
                Some(AgentDecl::OpenCode { command, args, env })
            }
            "generic" => Some(self.lower_generic_agent(kind, fields)),
            other => {
                self.errors.push(
                    Diagnostic::error(kind.span.clone(), format!("unknown agent kind `{other}`"))
                        .with_hint(
                            "valid kinds: claude, codex, gemini, antigravity, copilot, cursor, cline, opencode, generic",
                        ),
                );
                None
            }
        }
    }

    pub(super) fn parse_agent_mode(&mut self, name: &str, span: Span) -> Option<AgentMode> {
        match name {
            "interactive" => Some(AgentMode::Interactive),
            "print" => Some(AgentMode::Print),
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown agent mode `{other}`"))
                        .with_hint("valid modes: interactive, print"),
                );
                None
            }
        }
    }

    pub(super) fn parse_clone_apply_back(
        &mut self,
        name: &str,
        span: Span,
    ) -> Option<CloneApplyBackMode> {
        match name {
            "sync" => Some(CloneApplyBackMode::Sync),
            "discard" => Some(CloneApplyBackMode::Discard),
            "merge" => Some(CloneApplyBackMode::Merge),
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown apply_back mode `{other}`"))
                        .with_hint("valid modes: sync, discard, merge"),
                );
                None
            }
        }
    }
}
