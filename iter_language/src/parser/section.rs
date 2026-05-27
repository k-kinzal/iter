//! Top-level section dispatch: file, block-style sections (queue / workspace /
//! agent / trigger / runner) and their shared block parser.

use super::Parser;
use super::cst::{RawBlock, RawField, RawFile, RawIdent, RawSection, RawValue};
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_file(&mut self) -> RawFile {
        let mut sections = Vec::new();
        while self.pos < self.tokens.len() {
            let saved = self.pos;
            match self.peek() {
                Some(Token::Ident(name)) => match name.as_str() {
                    "prompt" => {
                        if let Some(section) = self.parse_prompt_section() {
                            sections.push(section);
                        } else {
                            if self.pos == saved {
                                self.bump();
                            }
                            self.recover_to_top_level();
                        }
                    }
                    "on" => {
                        if let Some(section) = self.parse_top_on_section() {
                            sections.push(section);
                        } else {
                            if self.pos == saved {
                                self.bump();
                            }
                            self.recover_to_top_level();
                        }
                    }
                    "arg" => {
                        if let Some(section) = self.parse_arg_section() {
                            sections.push(section);
                        } else {
                            if self.pos == saved {
                                self.bump();
                            }
                            self.recover_to_top_level();
                        }
                    }
                    _ => {
                        // Generic block section: the parser does not know which
                        // keywords are valid in which file kind (Iterfile vs
                        // compose.iter). Domain dispatch is the semantic
                        // layer's job — it consults the keyword and produces
                        // a "unknown top-level keyword" diagnostic when
                        // appropriate.
                        let _ = name;
                        if let Some(section) = self.parse_block_section() {
                            sections.push(section);
                        } else {
                            if self.pos == saved {
                                self.bump();
                            }
                            self.recover_to_top_level();
                        }
                    }
                },
                Some(_) => {
                    let span = self.peek_span();
                    let got = self.peek().map(Token::describe).unwrap_or_default();
                    self.errors.push(Diagnostic::error(
                        span,
                        format!("unexpected {got} at top level"),
                    ));
                    self.bump();
                    self.recover_to_top_level();
                }
                None => break,
            }
        }
        RawFile { sections }
    }

    pub(super) fn parse_block_section(&mut self) -> Option<RawSection> {
        // Only consume a second ident if it is immediately followed by `{`
        // *and* it is not one of the reserved top-level section keywords.
        // The lexer drops newlines, so `queue memory\nrunner { ... }` would
        // otherwise look indistinguishable from `queue memory runner { ... }`
        // — the keyword blacklist is the disambiguator. The list is
        // intentionally narrow: only words that cannot legitimately appear
        // as a kind in compose.iter are excluded.
        const RESERVED_SECTION_KEYWORDS: &[&str] = &[
            "queue",
            "workspace",
            "agent",
            "trigger",
            "runner",
            "service",
            "telemetry",
            "prompt",
            "on",
            "arg",
            "as",
        ];
        let keyword_tok = self.bump()?.clone();
        let Token::Ident(keyword) = keyword_tok.token else {
            return None;
        };
        let keyword_span = keyword_tok.span;
        // `runner` and compose-level singleton blocks take no leading ident;
        // everything else requires at least one. compose.iter uses two
        // leading idents (`queue main file { ... }`, `trigger nightly cron {
        // ... }`); the parser captures both and the semantic layer decides
        // whether the second is allowed.
        let kind = if matches!(keyword.as_str(), "runner" | "telemetry") {
            None
        } else {
            self.expect_ident()
        };
        // `as <name>` — Iterfile naming clause:
        //   `agent claude as primary { ... }`
        // The alias is captured here; the semantic layer decides whether
        // it is valid (Iterfile) or rejected (compose.iter).
        let alias = if matches!(self.peek(), Some(Token::Ident(name)) if name == "as") {
            self.bump(); // consume `as`
            self.expect_ident()
        } else {
            None
        };
        let kind2 = if alias.is_none() {
            if let Some(Token::Ident(name)) = self.peek()
                && !RESERVED_SECTION_KEYWORDS.contains(&name.as_str())
                && matches!(self.peek_at(1), Some(Token::LBrace))
            {
                self.expect_ident()
            } else {
                None
            }
        } else {
            None
        };
        let mut span_end = self.last_span().end;
        let body = if matches!(self.peek(), Some(Token::LBrace)) {
            let block = self.parse_block();
            span_end = block.span.end;
            Some(block)
        } else {
            None
        };
        Some(RawSection::Block {
            keyword,
            keyword_span: keyword_span.clone(),
            kind,
            kind2,
            alias,
            body,
            span: keyword_span.start..span_end,
        })
    }

    /// Parse `arg <name> [= "<default>"]`.
    ///
    /// Produces a [`RawSection::Block`] with keyword `"arg"`, kind set to
    /// the argument name, and an optional body carrying a single `default`
    /// field when a `= "value"` follows the name.
    pub(super) fn parse_arg_section(&mut self) -> Option<RawSection> {
        let keyword_tok = self.bump()?.clone();
        let keyword_span = keyword_tok.span;
        let name = self.expect_ident()?;
        let mut span_end = name.span.end;
        let body = if matches!(self.peek(), Some(Token::Equals)) {
            self.bump();
            let (value, value_span) = self.expect_string()?;
            span_end = value_span.end;
            let field_span = name.span.start..value_span.end;
            Some(RawBlock {
                fields: vec![RawField {
                    name: RawIdent {
                        name: "default".to_string(),
                        span: value_span.clone(),
                    },
                    value: RawValue::String(value, value_span),
                    span: field_span,
                }],
                routes: Vec::new(),
                actions: Vec::new(),
                span: name.span.start..span_end,
            })
        } else {
            None
        };
        Some(RawSection::Block {
            keyword: "arg".to_string(),
            keyword_span: keyword_span.clone(),
            kind: Some(name),
            kind2: None,
            alias: None,
            body,
            span: keyword_span.start..span_end,
        })
    }

    pub(super) fn parse_block(&mut self) -> RawBlock {
        let lbrace_span = self.peek_span();
        if !self.expect(&Token::LBrace, "`{`") {
            return RawBlock {
                fields: Vec::new(),
                routes: Vec::new(),
                actions: Vec::new(),
                span: lbrace_span,
            };
        }
        let start = lbrace_span.start;
        let mut fields = Vec::new();
        let mut routes = Vec::new();
        let mut actions = Vec::new();

        loop {
            match self.peek() {
                Some(Token::RBrace) => {
                    let end = self.peek_span().end;
                    self.bump();
                    return RawBlock {
                        fields,
                        routes,
                        actions,
                        span: start..end,
                    };
                }
                None => {
                    self.errors.push(Diagnostic::error(
                        self.eof_span(),
                        "unexpected end of file inside block; expected `}`",
                    ));
                    return RawBlock {
                        fields,
                        routes,
                        actions,
                        span: start..self.source_len,
                    };
                }
                Some(Token::Ident(name)) => match name.as_str() {
                    "on" => {
                        // Nested webhook route: on "<pattern>" [when "<str>"] { ... }
                        if let Some(route) = self.parse_nested_route() {
                            routes.push(route);
                        } else {
                            self.recover_inside_block();
                        }
                    }
                    "shell" => {
                        // `shell` is dual-purpose: an action shorthand
                        // (`shell "<cmd>"`) inside event handlers and a
                        // plain field (`shell = "bash -c"`) inside
                        // `trigger command` blocks. Disambiguate by
                        // peeking past the keyword.
                        if matches!(self.peek_at(1), Some(Token::Equals)) {
                            if let Some(field) = self.parse_field() {
                                fields.push(field);
                            } else {
                                self.recover_inside_block();
                            }
                        } else if let Some(action) = self.parse_action() {
                            actions.push(action);
                        } else {
                            self.recover_inside_block();
                        }
                    }
                    _ => {
                        if let Some(field) = self.parse_field() {
                            fields.push(field);
                        } else {
                            self.recover_inside_block();
                        }
                    }
                },
                // String-keyed field (e.g. Kafka headers `"x-source" = "..."`).
                Some(Token::String(_)) => {
                    if let Some(field) = self.parse_field() {
                        fields.push(field);
                    } else {
                        self.recover_inside_block();
                    }
                }
                Some(_) => {
                    let span = self.peek_span();
                    let got = self.peek().map(Token::describe).unwrap_or_default();
                    self.errors.push(Diagnostic::error(
                        span,
                        format!("expected a field, action, or route, found {got}"),
                    ));
                    self.recover_inside_block();
                }
            }
        }
    }
}
