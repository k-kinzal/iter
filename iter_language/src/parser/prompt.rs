//! `prompt`, top-level `on`, nested route, and `shell` action parsers.

use super::Parser;
use super::cst::{RawAction, RawEventHandler, RawPromptMatchArm, RawRoute, RawSection, RawValue};
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_prompt_section(&mut self) -> Option<RawSection> {
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
        Some(RawSection::Prompt {
            keyword_span: keyword_span.clone(),
            name,
            guard,
            body,
            body_span: body_span.clone(),
            span: keyword_span.start..body_span.end,
        })
    }

    pub(super) fn parse_top_on_section(&mut self) -> Option<RawSection> {
        let keyword_span = self.peek_span();
        self.bump(); // `on`
        let event = self.expect_ident()?;
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(RawSection::On {
            keyword_span: keyword_span.clone(),
            event,
            body,
            span: keyword_span.start..span_end,
        })
    }

    pub(super) fn parse_nested_route(&mut self) -> Option<RawRoute> {
        let on_span = self.peek_span();
        self.bump(); // `on`
        let (pattern, _pat_span) = self.expect_string()?;
        let when = if matches!(self.peek(), Some(Token::Ident(name)) if name == "when") {
            self.bump();
            let (s, _) = self.expect_string()?;
            Some(s)
        } else {
            None
        };
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(RawRoute {
            event_pattern: pattern,
            when,
            body,
            span: on_span.start..span_end,
        })
    }

    pub(super) fn parse_action(&mut self) -> Option<RawAction> {
        let kw_span = self.peek_span();
        self.bump(); // `shell`
        let (cmd, cmd_span) = self.expect_string()?;
        Some(RawAction {
            keyword_span: kw_span.clone(),
            command: cmd,
            span: kw_span.start..cmd_span.end,
        })
    }

    /// Parse `_ => <value>` — prompt match default arm.
    pub(super) fn parse_prompt_match_default_arm(&mut self) -> Option<RawPromptMatchArm> {
        let start = self.peek_span().start;
        self.bump(); // `_`
        self.bump(); // `=>`
        let value = self.parse_prompt_arm_value()?;
        let end = value.span().end;
        Some(RawPromptMatchArm {
            guard: None,
            value,
            span: start..end,
        })
    }

    /// Parse `<guard-expr> => <value>` — guarded prompt match arm.
    pub(super) fn parse_prompt_match_guard_arm(&mut self) -> Option<RawPromptMatchArm> {
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
        Some(RawPromptMatchArm {
            guard: Some(guard),
            value,
            span: start..end,
        })
    }

    /// Parse `on <ident> { <actions> }` — nested event handler inside a block.
    pub(super) fn parse_nested_event_handler(&mut self) -> Option<RawEventHandler> {
        let on_span = self.peek_span();
        self.bump(); // `on`
        let event = self.expect_ident()?;
        let body = self.parse_block();
        let span_end = body.span.end;
        Some(RawEventHandler {
            event,
            body,
            span: on_span.start..span_end,
        })
    }

    fn parse_prompt_arm_value(&mut self) -> Option<RawValue> {
        match self.peek() {
            Some(Token::String(s)) => {
                let s = s.clone();
                let span = self.peek_span();
                self.bump();
                Some(RawValue::String(s, span))
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                let span = self.peek_span();
                self.bump();
                Some(RawValue::Ident(name, span))
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
