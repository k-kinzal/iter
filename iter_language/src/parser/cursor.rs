//! Token-stream cursor helpers and error-recovery routines.

use super::Parser;
use super::cst::RawIdent;
use crate::ast::Span;
use crate::diagnostic::Diagnostic;
use crate::lexer::{SpannedToken, Token};

impl<'a> Parser<'a> {
    pub(super) fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos).map(|t| &t.token)
    }

    pub(super) fn peek_at(&self, offset: usize) -> Option<&'a Token> {
        self.tokens.get(self.pos + offset).map(|t| &t.token)
    }

    pub(super) fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|t| t.span.clone())
            .unwrap_or(self.eof_span())
    }

    pub(super) fn eof_span(&self) -> Span {
        let s = self.source_len;
        s..s
    }

    pub(super) fn bump(&mut self) -> Option<&'a SpannedToken> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    pub(super) fn last_span(&self) -> Span {
        if self.pos == 0 {
            self.eof_span()
        } else {
            self.tokens[self.pos - 1].span.clone()
        }
    }

    pub(super) fn expect_ident(&mut self) -> Option<RawIdent> {
        match self.peek() {
            Some(Token::Ident(name)) => {
                let name = name.clone();
                let span = self.peek_span();
                self.bump();
                Some(RawIdent { name, span })
            }
            // Treat keywords-as-words by name when used in identifier
            // position. The lexer keeps `true`/`false` as their own tokens;
            // here we accept them as valid bareword idents because users may
            // legitimately write `interactive`, `print`, `normal`, etc.
            Some(Token::True) => {
                let span = self.peek_span();
                self.bump();
                Some(RawIdent {
                    name: "true".into(),
                    span,
                })
            }
            Some(Token::False) => {
                let span = self.peek_span();
                self.bump();
                Some(RawIdent {
                    name: "false".into(),
                    span,
                })
            }
            other => {
                let span = self.peek_span();
                let got = other.map_or_else(|| "end of file".to_string(), Token::describe);
                self.errors.push(Diagnostic::error(
                    span,
                    format!("expected an identifier, found {got}"),
                ));
                None
            }
        }
    }

    /// Field-name slot: identifier *or* string literal.
    ///
    /// String-literal field names support DSL surfaces where the natural map
    /// key isn't a legal identifier (Kafka header names with `-`, librdkafka
    /// extra-config keys with `.`). The lowerer decides per call site whether
    /// to accept the string-keyed shape; here we just parse it.
    pub(super) fn expect_field_name(&mut self) -> Option<RawIdent> {
        if let Some(Token::String(s)) = self.peek() {
            let name = s.clone();
            let span = self.peek_span();
            self.bump();
            return Some(RawIdent { name, span });
        }
        self.expect_ident()
    }

    pub(super) fn expect_string(&mut self) -> Option<(String, Span)> {
        match self.peek() {
            Some(Token::String(s)) => {
                let s = s.clone();
                let span = self.peek_span();
                self.bump();
                Some((s, span))
            }
            other => {
                let span = self.peek_span();
                let got = other.map_or_else(|| "end of file".to_string(), Token::describe);
                self.errors.push(Diagnostic::error(
                    span,
                    format!("expected a string literal, found {got}"),
                ));
                None
            }
        }
    }

    pub(super) fn expect(&mut self, expected: &Token, label: &str) -> bool {
        match self.peek() {
            Some(t) if t == expected => {
                self.bump();
                true
            }
            other => {
                let span = self.peek_span();
                let got = other.map_or_else(|| "end of file".to_string(), Token::describe);
                self.errors.push(Diagnostic::error(
                    span,
                    format!("expected {label}, found {got}"),
                ));
                false
            }
        }
    }

    /// Skip tokens until we hit a top-level recovery point (a known top-level
    /// keyword or end of file). Used after a fatal in-section error.
    pub(super) fn recover_to_top_level(&mut self) {
        while let Some(tok) = self.peek() {
            if matches!(
                tok,
                Token::Ident(name)
                if matches!(
                    name.as_str(),
                    "queue" | "workspace" | "agent" | "trigger" | "runner" | "prompt" | "on" | "arg"
                )
            ) {
                return;
            }
            self.bump();
        }
    }

    /// Skip until we reach a `}` or another likely statement starter so we
    /// can keep collecting errors inside a malformed block.
    pub(super) fn recover_inside_block(&mut self) {
        let mut depth: i32 = 0;
        while let Some(tok) = self.peek() {
            match tok {
                Token::RBrace if depth == 0 => return,
                Token::LBrace => {
                    depth += 1;
                    self.bump();
                }
                Token::RBrace => {
                    depth -= 1;
                    self.bump();
                }
                Token::Ident(name)
                    if depth == 0
                        && matches!(name.as_str(), "shell" | "on" | "metadata" | "iteration" | "queue" | "_") =>
                {
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }
}
