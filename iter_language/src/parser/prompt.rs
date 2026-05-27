//! `prompt`, top-level `on`, nested route, and `shell` action parsers.

use super::Parser;
use super::cst::{RawAction, RawRoute, RawSection};
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
        Some(RawSection::On {
            keyword_span: keyword_span.clone(),
            event,
            body: body.clone(),
            span: keyword_span.start..body.span.end,
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
        Some(RawRoute {
            event_pattern: pattern,
            when,
            body: body.clone(),
            span: on_span.start..body.span.end,
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
}
