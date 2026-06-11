//! Value-position parsing: fields, scalar/composite values, lists, and calls.

use super::Parser;
use super::cst::{CstField, CstValue};
use crate::ast::Span;
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_field(&mut self) -> Option<CstField> {
        let name = self.expect_field_name()?;
        // Two forms:
        //   <name> = value
        //   <name> { ... }    // shorthand for nested block (e.g. metadata { kind = "x" })
        let value = if matches!(self.peek(), Some(Token::LBrace)) {
            let block = self.parse_block();
            CstValue::Block(block)
        } else {
            if !self.expect(&Token::Equals, "`=`") {
                return None;
            }
            self.parse_value()?
        };
        let span = name.span.start..value.span().end;
        Some(CstField { name, value, span })
    }

    pub(super) fn parse_value(&mut self) -> Option<CstValue> {
        let tok = self.tokens.get(self.pos)?.clone();
        match tok.token {
            Token::String(s) => {
                self.bump();
                Some(CstValue::String(s, tok.span))
            }
            Token::Integer(n) => {
                self.bump();
                Some(CstValue::Integer(n, tok.span))
            }
            Token::Duration(secs) => {
                self.bump();
                Some(CstValue::Duration(secs, tok.span))
            }
            Token::True => {
                self.bump();
                Some(CstValue::Bool(true, tok.span))
            }
            Token::False => {
                self.bump();
                Some(CstValue::Bool(false, tok.span))
            }
            Token::Null => {
                self.bump();
                Some(CstValue::Null(tok.span))
            }
            Token::LBracket => self.parse_list(),
            Token::LBrace => Some(CstValue::Block(self.parse_block())),
            Token::Ident(name) => {
                if matches!(self.peek_at(1), Some(Token::LParen)) {
                    self.parse_call(name, tok.span)
                } else {
                    self.bump();
                    Some(CstValue::Ident(name, tok.span))
                }
            }
            _ => {
                let got = tok.token.describe();
                self.errors.push(Diagnostic::error(
                    tok.span,
                    format!("expected a value, found {got}"),
                ));
                None
            }
        }
    }

    pub(super) fn parse_list(&mut self) -> Option<CstValue> {
        let lspan = self.peek_span();
        if !self.expect(&Token::LBracket, "`[`") {
            return None;
        }
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some(Token::RBracket) => {
                    let end = self.peek_span().end;
                    self.bump();
                    return Some(CstValue::List(items, lspan.start..end));
                }
                None => {
                    self.errors.push(Diagnostic::error(
                        self.eof_span(),
                        "unexpected end of file inside list; expected `]`",
                    ));
                    return Some(CstValue::List(items, lspan.start..self.source_len));
                }
                _ => {
                    if let Some(v) = self.parse_value() {
                        items.push(v);
                    } else {
                        // skip until comma or ]
                        while let Some(t) = self.peek() {
                            if matches!(t, Token::Comma | Token::RBracket) {
                                break;
                            }
                            self.bump();
                        }
                    }
                    match self.peek() {
                        Some(Token::Comma) => {
                            self.bump();
                        }
                        Some(Token::RBracket) => {}
                        _ => {
                            let span = self.peek_span();
                            let got = self.peek().map(Token::describe).unwrap_or_default();
                            self.errors.push(Diagnostic::error(
                                span,
                                format!("expected `,` or `]` in list, found {got}"),
                            ));
                        }
                    }
                }
            }
        }
    }

    pub(super) fn parse_call(&mut self, name: String, name_span: Span) -> Option<CstValue> {
        // Consume name + (
        self.bump();
        self.bump();
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Token::RParen) => {
                    let end = self.peek_span().end;
                    self.bump();
                    return Some(CstValue::Call {
                        name,
                        args,
                        span: name_span.start..end,
                    });
                }
                None => {
                    self.errors.push(Diagnostic::error(
                        self.eof_span(),
                        "unexpected end of file inside call; expected `)`",
                    ));
                    return Some(CstValue::Call {
                        name,
                        args,
                        span: name_span.start..self.source_len,
                    });
                }
                _ => {
                    if let Some(v) = self.parse_value() {
                        args.push(v);
                    } else {
                        while let Some(t) = self.peek() {
                            if matches!(t, Token::Comma | Token::RParen) {
                                break;
                            }
                            self.bump();
                        }
                    }
                    match self.peek() {
                        Some(Token::Comma) => {
                            self.bump();
                        }
                        Some(Token::RParen) => {}
                        _ => {
                            let span = self.peek_span();
                            let got = self.peek().map(Token::describe).unwrap_or_default();
                            self.errors.push(Diagnostic::error(
                                span,
                                format!("expected `,` or `)` in function call, found {got}"),
                            ));
                        }
                    }
                }
            }
        }
    }
}
