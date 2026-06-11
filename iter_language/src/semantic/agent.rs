//! `agent { ... }` lowerer plus mode/apply-back identifier parsers shared with field helpers.

use super::{Analyzer, COMMAND_HINT};
use crate::ast::{AgentDef, AgentMode, CloneApplyBackMode, RouterStrategy, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstIdent, CstValue};

struct SimpleAgentParts {
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
}

impl Analyzer {
    pub(super) fn lower_agent(
        &mut self,
        kind: Option<CstIdent>,
        body: Option<CstBlock>,
        keyword_span: &Span,
    ) -> Option<AgentDef> {
        let kind = self.require_kind(
            kind,
            keyword_span,
            "agent",
            &[
                "claude",
                "codex",
                "gemini",
                "hermes",
                "antigravity",
                "copilot",
                "cursor",
                "cline",
                "opencode",
                "grok",
                "generic",
                "noop",
                "fake",
                "router",
            ],
        )?;
        if kind.name == "router" {
            return self.lower_router_agent(&kind, body);
        }
        let mut fields = self.collect_fields(body);
        let decl = match kind.name.as_str() {
            "claude" | "codex" | "gemini" | "hermes" | "antigravity" | "copilot" => {
                self.lower_mode_agent(&kind, &mut fields)?
            }
            "cursor" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "cursor")?;
                AgentDef::Cursor { command, args, env }
            }
            "cline" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "cline")?;
                AgentDef::Cline { command, args, env }
            }
            "opencode" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(&kind, &mut fields, "opencode")?;
                AgentDef::OpenCode { command, args, env }
            }
            "grok" => self.lower_grok_agent(&kind, &mut fields)?,
            "generic" => self.lower_generic_agent(&kind, &mut fields),
            "noop" => {
                self.reject_unknown_fields(&mut fields, &[], "agent noop");
                AgentDef::Noop
            }
            "fake" => self.lower_fake_agent(&kind, &mut fields),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind.span,
                        format!("unknown agent kind `{other}`"),
                    )
                    .with_hint(
                        "valid kinds: claude, codex, gemini, hermes, antigravity, copilot, cursor, cline, opencode, grok, generic, noop, fake, router",
                    ),
                );
                return None;
            }
        };
        Some(decl)
    }

    fn lower_mode_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
    ) -> Option<AgentDef> {
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
                Some(AgentDef::Claude {
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
                Some(AgentDef::Codex {
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
                Some(AgentDef::Gemini {
                    mode: mode?,
                    command: command?,
                    args,
                    env,
                })
            }
            "hermes" => {
                self.reject_unknown_fields(
                    fields,
                    &["mode", "command", "args", "env"],
                    "agent hermes",
                );
                Some(AgentDef::Hermes {
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
                Some(AgentDef::Antigravity {
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
                Some(AgentDef::Copilot {
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
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
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

    fn lower_grok_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
    ) -> Option<AgentDef> {
        let command = self.take_required_string_with_hint(
            fields,
            "command",
            &kind.span,
            "agent grok",
            COMMAND_HINT,
        );
        let args = self
            .take_optional_string_list(fields, "args")
            .unwrap_or_default();
        let session_id_file = self.take_optional_string(fields, "session_id_file");
        let env = self.take_optional_env_block(fields);
        self.reject_unknown_fields(
            fields,
            &["command", "args", "session_id_file", "env"],
            "agent grok",
        );
        Some(AgentDef::Grok {
            command: command?,
            args,
            session_id_file,
            env,
        })
    }

    fn lower_generic_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
    ) -> AgentDef {
        let command = if let Some(field) = fields.remove("command") {
            match field.value {
                CstValue::List(items, _) => {
                    let mut out = Vec::new();
                    for item in items {
                        match item {
                            CstValue::String(s, _) => out.push(s),
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
        AgentDef::Generic { command, env }
    }

    fn lower_fake_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
    ) -> AgentDef {
        #[allow(clippy::cast_possible_truncation)]
        let exit_code = self
            .take_optional_int(fields, "exit_code")
            .map_or(0, |n| n as i32);
        let delay_secs = self.take_optional_u64(fields, "delay_secs");
        let stdout = self
            .take_optional_string_list(fields, "stdout")
            .unwrap_or_default();
        let stderr = self
            .take_optional_string_list(fields, "stderr")
            .unwrap_or_default();
        let files = self.take_optional_string_kv_block(fields, "files");
        self.reject_unknown_fields(
            fields,
            &["exit_code", "delay_secs", "stdout", "stderr", "files"],
            &format!("agent {}", kind.name),
        );
        AgentDef::Fake {
            exit_code,
            delay_secs,
            stdout,
            stderr,
            files,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower_router_agent(&mut self, kind: &CstIdent, body: Option<CstBlock>) -> Option<AgentDef> {
        let raw_fields = match body {
            Some(block) => block.fields,
            None => Vec::new(),
        };

        let mut strategy = RouterStrategy::Fallback;
        let mut agents: Vec<(String, Box<AgentDef>)> = Vec::new();
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
                    CstValue::Ident(ref ident, ref span) => match ident.as_str() {
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
                CstValue::Block(block) => {
                    let mut sub_fields = self.collect_fields(Some(block));
                    let sub_kind = if let Some(kind_field) = sub_fields.remove("kind") {
                        match kind_field.value {
                            CstValue::Ident(s, _) => s,
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
                            .with_hint(
                                "add `kind = claude` (or codex, gemini, grok, opencode, etc.)",
                            ),
                        );
                        continue;
                    };
                    let sub_ident = CstIdent {
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

        Some(AgentDef::Router { agents, strategy })
    }

    fn lower_sub_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, crate::parser::CstField>,
    ) -> Option<AgentDef> {
        match kind.name.as_str() {
            "claude" | "codex" | "gemini" | "hermes" | "antigravity" | "copilot" => {
                self.lower_mode_agent(kind, fields)
            }
            "cursor" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "cursor")?;
                Some(AgentDef::Cursor { command, args, env })
            }
            "cline" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "cline")?;
                Some(AgentDef::Cline { command, args, env })
            }
            "opencode" => {
                let SimpleAgentParts { command, args, env } =
                    self.lower_simple_agent(kind, fields, "opencode")?;
                Some(AgentDef::OpenCode { command, args, env })
            }
            "grok" => self.lower_grok_agent(kind, fields),
            "generic" => Some(self.lower_generic_agent(kind, fields)),
            "noop" => {
                self.reject_unknown_fields(fields, &[], "agent noop");
                Some(AgentDef::Noop)
            }
            "fake" => Some(self.lower_fake_agent(kind, fields)),
            other => {
                self.errors.push(
                    Diagnostic::error(kind.span.clone(), format!("unknown agent kind `{other}`"))
                        .with_hint(
                            "valid kinds: claude, codex, gemini, hermes, antigravity, copilot, cursor, cline, opencode, grok, generic, noop, fake",
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
