//! `agent { ... }` lowerer plus mode/apply-back identifier parsers shared with field helpers.

use super::{Analyzer, COMMAND_HINT};
use crate::ast::{AgentDecl, AgentMode, CloneApplyBackMode, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawIdent, RawValue};

struct SimpleAgentParts {
    command: String,
    args: Vec<String>,
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
                "claude", "codex", "gemini", "copilot", "cursor", "cline", "opencode", "generic",
            ],
        )?;
        let mut fields = self.collect_fields(body);
        let decl = match kind.name.as_str() {
            "claude" | "codex" | "gemini" | "copilot" => {
                self.lower_mode_agent(&kind, &mut fields)?
            }
            "cursor" => {
                let SimpleAgentParts { command, args } =
                    self.lower_simple_agent(&kind, &mut fields, "cursor")?;
                AgentDecl::Cursor { command, args }
            }
            "cline" => {
                let SimpleAgentParts { command, args } =
                    self.lower_simple_agent(&kind, &mut fields, "cline")?;
                AgentDecl::Cline { command, args }
            }
            "opencode" => {
                let SimpleAgentParts { command, args } =
                    self.lower_simple_agent(&kind, &mut fields, "opencode")?;
                AgentDecl::OpenCode { command, args }
            }
            "generic" => self.lower_generic_agent(&kind, &mut fields),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind.span,
                        format!("unknown agent kind `{other}`"),
                    )
                    .with_hint(
                        "valid kinds: claude, codex, gemini, copilot, cursor, cline, opencode, generic",
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
        match kind.name.as_str() {
            "claude" => {
                let session_id_file = self.take_optional_string(fields, "session_id_file");
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "session_id_file"],
                    "agent claude",
                );
                Some(AgentDecl::Claude {
                    mode: mode?,
                    command: command?,
                    args,
                    session_id_file,
                })
            }
            "codex" => {
                self.reject_unknown_fields(fields, &["mode", "command", "args"], "agent codex");
                Some(AgentDecl::Codex {
                    mode: mode?,
                    command: command?,
                    args,
                })
            }
            "gemini" => {
                self.reject_unknown_fields(fields, &["mode", "command", "args"], "agent gemini");
                Some(AgentDecl::Gemini {
                    mode: mode?,
                    command: command?,
                    args,
                })
            }
            "copilot" => {
                let subcommand = self.take_optional_string_list(fields, "subcommand");
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "subcommand", "args"],
                    "agent copilot",
                );
                Some(AgentDecl::Copilot {
                    mode: mode?,
                    command: command?,
                    subcommand,
                    args,
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
        self.reject_unknown_fields(fields, &["command", "args"], &format!("agent {label}"));
        Some(SimpleAgentParts {
            command: command?,
            args,
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
        self.reject_unknown_fields(fields, &["command"], "agent generic");
        AgentDecl::Generic { command }
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
