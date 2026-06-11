//! `prompt`, top-level `on`, nested route, and `shell` action parsers.

use super::Parser;
use super::cst::{CstAction, CstEventHandler, CstPromptMatchArm, CstRoute, CstSection, CstValue};
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_prompt_section(&mut self) -> Option<CstSection> {
        let keyword_span = self.peek_span();
        self.bump(); // `prompt`
        // `prompt as <name> "..."` — named prompt definition.
        let name = if matches!(self.peek(), Some(Token::Ident(name)) if name == "as") {
            self.bump(); // consume `as`
            self.expect_ident()
        } else {
            None
        };
        let guard = if name.is_none()
            && matches!(self.peek(), Some(Token::Ident(name)) if name == "when")
        {
            self.bump();
            self.parse_guard()
        } else {
            None
        };
        let (body, body_span) = self.expect_string()?;
        Some(CstSection::Prompt {
            keyword_span: keyword_span.clone(),
            name,
            guard,
            body,
            body_span: body_span.clone(),
            span: keyword_span.start..body_span.end,
        })
    }

    pub(super) fn parse_top_on_section(&mut self) -> Option<CstSection> {
        let keyword_span = self.peek_span();
        self.bump(); // `on`
        let event = self.expect_ident()?;
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(CstSection::On {
            keyword_span: keyword_span.clone(),
            event,
            body,
            span: keyword_span.start..span_end,
        })
    }

    pub(super) fn parse_nested_route(&mut self) -> Option<CstRoute> {
        let on_span = self.peek_span();
        self.bump(); // `on`
        let (pattern, _pat_span) = self.expect_string()?;
        let (when, when_span) = if matches!(self.peek(), Some(Token::Ident(name)) if name == "when")
        {
            self.bump();
            let (s, sp) = self.expect_string()?;
            (Some(s), Some(sp))
        } else {
            (None, None)
        };
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(CstRoute {
            event_pattern: pattern,
            when,
            when_span,
            body,
            span: on_span.start..span_end,
        })
    }

    pub(super) fn parse_action(&mut self) -> Option<CstAction> {
        let kw_span = self.peek_span();
        self.bump(); // `shell`
        let (cmd, cmd_span) = self.expect_string()?;
        Some(CstAction {
            keyword_span: kw_span.clone(),
            command: cmd,
            span: kw_span.start..cmd_span.end,
        })
    }

    /// Parse `_ => <value>` — prompt match default arm.
    pub(super) fn parse_prompt_match_default_arm(&mut self) -> Option<CstPromptMatchArm> {
        let start = self.peek_span().start;
        self.bump(); // `_`
        self.bump(); // `=>`
        let value = self.parse_prompt_arm_value()?;
        let end = value.span().end;
        Some(CstPromptMatchArm {
            guard: None,
            value,
            span: start..end,
        })
    }

    /// Parse `<guard-expr> => <value>` — guarded prompt match arm.
    pub(super) fn parse_prompt_match_guard_arm(&mut self) -> Option<CstPromptMatchArm> {
        let start = self.peek_span().start;
        let guard = self.parse_guard()?;
        if !matches!(self.peek(), Some(Token::FatArrow)) {
            let span = self.peek_span();
            let got = self.peek().map(Token::describe).unwrap_or_default();
            self.errors.push(Diagnostic::error(
                span,
                format!("expected `=>` after guard expression, found {got}"),
            ));
            return None;
        }
        self.bump(); // `=>`
        let value = self.parse_prompt_arm_value()?;
        let end = value.span().end;
        Some(CstPromptMatchArm {
            guard: Some(guard),
            value,
            span: start..end,
        })
    }

    /// Parse `on <ident> { <actions> }` — nested event handler inside a block.
    pub(super) fn parse_nested_event_handler(&mut self) -> Option<CstEventHandler> {
        let on_span = self.peek_span();
        self.bump(); // `on`
        let event = self.expect_ident()?;
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(CstEventHandler {
            event,
            body,
            span: on_span.start..span_end,
        })
    }

    fn parse_prompt_arm_value(&mut self) -> Option<CstValue> {
        match self.peek() {
            Some(Token::String(s)) => {
                let s = s.clone();
                let span = self.peek_span();
                self.bump();
                Some(CstValue::String(s, span))
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                let span = self.peek_span();
                self.bump();
                Some(CstValue::Ident(name, span))
            }
            other => {
                let span = self.peek_span();
                let got = other.map_or_else(|| "end of file".to_string(), Token::describe);
                self.errors.push(Diagnostic::error(
                    span,
                    format!("expected a string or name reference after `=>`, found {got}"),
                ));
                None
            }
        }
    }
}
