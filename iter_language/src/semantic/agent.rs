//! `agent { ... }` lowerer plus mode/apply-back identifier parsers shared with field helpers.

use super::{Analyzer, COMMAND_HINT};
use crate::ast::{
    AgentDef, AgentMode, CloneApplyBackMode, RouterFallbackClass, RouterFallbackTriggers,
    RouterStrategy, Span,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstIdent, CstValue};

struct SimpleAgentParts {
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
}

impl Analyzer {
    fn default_command_for_agent(kind: &str) -> Option<&'static str> {
        match kind {
            "claude" => Some("claude"),
            "codex" => Some("codex"),
            "gemini" => Some("gemini"),
            "hermes" => Some("hermes"),
            "antigravity" => Some("agy"),
            "copilot" => Some("gh"),
            "grok" => Some("grok"),
            "cursor" => Some("cursor"),
            "cline" => Some("cline"),
            "opencode" => Some("opencode"),
            _ => None,
        }
    }

    fn take_agent_command_or_default(
        &mut self,
        fields: &mut std::collections::BTreeMap<String, CstField>,
        kind: &str,
    ) -> Option<String> {
        if fields.contains_key("command") {
            return self.take_optional_string(fields, "command");
        }
        Self::default_command_for_agent(kind).map(str::to_owned)
    }

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
        fields: &mut std::collections::BTreeMap<String, CstField>,
    ) -> Option<AgentDef> {
        let mode = self.take_required_agent_mode(fields, &kind.span, &kind.name);
        let command = self.take_agent_command_or_default(fields, &kind.name);
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
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind.span.clone(),
                        format!("unknown mode-capable agent kind `{other}`"),
                    )
                    .with_hint("valid mode-capable kinds: claude, codex, gemini, hermes, antigravity, copilot"),
                );
                None
            }
        }
    }

    fn lower_simple_agent(
        &mut self,
        _kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, CstField>,
        label: &str,
    ) -> Option<SimpleAgentParts> {
        let command = self.take_agent_command_or_default(fields, label);
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
        fields: &mut std::collections::BTreeMap<String, CstField>,
    ) -> Option<AgentDef> {
        let command = self.take_agent_command_or_default(fields, &kind.name);
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
        fields: &mut std::collections::BTreeMap<String, CstField>,
    ) -> AgentDef {
        let command = if let Some(field) = fields.remove("command") {
            match field.value {
                CstValue::List(items, _) => {
                    let mut out = Vec::new();
                    for item in items {
                        match item {
                            CstValue::String(s, _) => out.push(s),
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
                                    "`command` list elements must be strings",
                                ));
                            }
                        }
                    }
                    out
                }
                other @ (CstValue::String(..)
                | CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Bool(..)
                | CstValue::Null(_)
                | CstValue::Ident(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
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
                    .with_hint(COMMAND_HINT),
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
        fields: &mut std::collections::BTreeMap<String, CstField>,
    ) -> AgentDef {
        let exit_code = self
            .take_optional_int(fields, "exit_code")
            .map_or(0, |n| i32::try_from(n).unwrap_or(0));
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
    fn lower_router_agent(&mut self, kind: &CstIdent, body: Option<CstBlock>) -> Option<AgentDef> {
        let raw_fields = match body {
            Some(block) => block.fields,
            None => Vec::new(),
        };

        let mut strategy = RouterStrategy::Fallback;
        let mut fallback_on = RouterFallbackTriggers::Any;
        let mut fallback_on_span = None;
        let mut agents: Vec<(String, Box<AgentDef>)> = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for field in raw_fields {
            let name = field.name.name.clone();

            if name == "strategy" {
                self.lower_router_strategy(field, &mut seen_names, &mut strategy)?;
                continue;
            }

            if name == "fallback_on" {
                self.lower_router_fallback_on(
                    field,
                    &mut seen_names,
                    &mut fallback_on,
                    &mut fallback_on_span,
                )?;
                continue;
            }

            if !seen_names.insert(name.clone()) {
                self.errors.push(Diagnostic::error(
                    field.name.span.clone(),
                    format!("duplicate field `{name}` in block"),
                ));
                continue;
            }

            self.lower_router_sub_agent(field, name, &mut agents);
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

        if strategy == RouterStrategy::Rotate
            && let Some(span) = fallback_on_span
        {
            self.errors.push(
                Diagnostic::error(
                    span,
                    "`fallback_on` is only valid with `strategy = fallback`",
                )
                .with_hint("remove `fallback_on` or set `strategy = fallback`"),
            );
            return None;
        }

        Some(AgentDef::Router {
            agents,
            strategy,
            fallback_on,
        })
    }

    fn lower_router_strategy(
        &mut self,
        field: CstField,
        seen_names: &mut std::collections::HashSet<String>,
        strategy: &mut RouterStrategy,
    ) -> Option<()> {
        if !seen_names.insert("strategy".to_string()) {
            self.errors.push(Diagnostic::error(
                field.name.span.clone(),
                "duplicate field `strategy` in block",
            ));
            return Some(());
        }
        match field.value {
            CstValue::Ident(ref ident, ref span) => match ident.as_str() {
                "fallback" => *strategy = RouterStrategy::Fallback,
                "rotate" => *strategy = RouterStrategy::Rotate,
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
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::List(..)
            | CstValue::Block(_)
            | CstValue::Call { .. }) => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`strategy` must be an identifier (fallback or rotate)",
                ));
                return None;
            }
        }
        Some(())
    }

    fn lower_router_fallback_on(
        &mut self,
        field: CstField,
        seen_names: &mut std::collections::HashSet<String>,
        fallback_on: &mut RouterFallbackTriggers,
        fallback_on_span: &mut Option<Span>,
    ) -> Option<()> {
        if !seen_names.insert("fallback_on".to_string()) {
            self.errors.push(Diagnostic::error(
                field.name.span.clone(),
                "duplicate field `fallback_on` in block",
            ));
            return Some(());
        }
        *fallback_on_span = Some(field.name.span.clone());
        *fallback_on = match field.value {
            CstValue::Ident(ref ident, _) if ident == "any" => RouterFallbackTriggers::Any,
            CstValue::Ident(ref ident, ref span) => {
                let class = self.parse_router_fallback_class(ident, span.clone())?;
                let mut classes = std::collections::BTreeSet::new();
                classes.insert(class);
                RouterFallbackTriggers::Only(classes)
            }
            CstValue::List(items, _) => {
                let mut classes = std::collections::BTreeSet::new();
                for item in items {
                    match item {
                        CstValue::Ident(ident, _) if ident == "any" => {
                            *fallback_on = RouterFallbackTriggers::Any;
                            return Some(());
                        }
                        CstValue::Ident(ident, span) => {
                            if let Some(class) = self.parse_router_fallback_class(&ident, span) {
                                classes.insert(class);
                            } else {
                                return None;
                            }
                        }
                        other @ (CstValue::String(..)
                        | CstValue::Integer(..)
                        | CstValue::Duration(..)
                        | CstValue::Bool(..)
                        | CstValue::Null(_)
                        | CstValue::List(..)
                        | CstValue::Block(_)
                        | CstValue::Call { .. }) => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                "`fallback_on` entries must be identifiers",
                            ));
                            return None;
                        }
                    }
                }
                RouterFallbackTriggers::Only(classes)
            }
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Block(_)
            | CstValue::Call { .. }) => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`fallback_on` must be `any` or a list of fallback classes",
                ));
                return None;
            }
        };
        Some(())
    }

    fn parse_router_fallback_class(
        &mut self,
        name: &str,
        span: Span,
    ) -> Option<RouterFallbackClass> {
        match name {
            "timeout" => Some(RouterFallbackClass::Timeout),
            "token_limit" => Some(RouterFallbackClass::TokenLimit),
            "errored" => Some(RouterFallbackClass::Launch),
            "terminated_by_signal" => Some(RouterFallbackClass::TerminatedBySignal),
            "failure" => Some(RouterFallbackClass::Failure),
            "cancelled" => {
                self.errors.push(
                    Diagnostic::error(span, "`cancelled` is never a fallback trigger")
                        .with_hint("cancellation is cooperative shutdown and always propagates"),
                );
                None
            }
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown fallback trigger `{other}`"))
                        .with_hint(
                            "valid fallback triggers: any, timeout, token_limit, errored, terminated_by_signal, failure",
                        ),
                );
                None
            }
        }
    }

    fn lower_router_sub_agent(
        &mut self,
        field: CstField,
        name: String,
        agents: &mut Vec<(String, Box<AgentDef>)>,
    ) {
        let field_span = field.name.span.clone();
        match field.value {
            CstValue::Block(block) => {
                let mut sub_fields = self.collect_fields(Some(block));
                let Some(sub_kind) =
                    self.take_router_sub_agent_kind(field_span.clone(), &name, &mut sub_fields)
                else {
                    return;
                };
                let sub_ident = CstIdent {
                    name: sub_kind,
                    span: field_span,
                };
                if let Some(decl) = self.lower_sub_agent(&sub_ident, &mut sub_fields) {
                    agents.push((name, Box::new(decl)));
                }
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
                    format!(
                        "router sub-agent `{name}` must be a block (`{name} {{ kind = ... }}`)"
                    ),
                ));
            }
        }
    }

    fn take_router_sub_agent_kind(
        &mut self,
        field_span: Span,
        name: &str,
        sub_fields: &mut std::collections::BTreeMap<String, CstField>,
    ) -> Option<String> {
        if let Some(kind_field) = sub_fields.remove("kind") {
            match kind_field.value {
                CstValue::Ident(s, _) => Some(s),
                other @ (CstValue::String(..)
                | CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Bool(..)
                | CstValue::Null(_)
                | CstValue::List(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`kind` must be an identifier",
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(
                    field_span,
                    format!("router sub-agent `{name}` requires `kind`"),
                )
                .with_hint("add `kind = claude` (or codex, gemini, grok, opencode, etc.)"),
            );
            None
        }
    }

    fn lower_sub_agent(
        &mut self,
        kind: &CstIdent,
        fields: &mut std::collections::BTreeMap<String, CstField>,
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
            "print" => Some(AgentMode::Headless),
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
