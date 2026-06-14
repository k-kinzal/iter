//! Guard-expression recursive descent (`||`, `&&`, `==`, `!=`, `<`/`<=`/`>`/`>=`,
//! `%`, parentheses).

use super::Parser;
use super::cst::{CstCmpOp, CstGuard};
use crate::ast::Span;
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_guard(&mut self) -> Option<CstGuard> {
        self.parse_guard_or()
    }

    pub(super) fn parse_guard_or(&mut self) -> Option<CstGuard> {
        let mut left = self.parse_guard_and()?;
        while matches!(self.peek(), Some(Token::PipePipe)) {
            self.bump();
            let right = self.parse_guard_and()?;
            let span = left.span().start..right.span().end;
            left = CstGuard::Or(Box::new(left), Box::new(right), span);
        }
        Some(left)
    }

    pub(super) fn parse_guard_and(&mut self) -> Option<CstGuard> {
        let mut left = self.parse_guard_atom()?;
        while matches!(self.peek(), Some(Token::AmpAmp)) {
            self.bump();
            let right = self.parse_guard_atom()?;
            let span = left.span().start..right.span().end;
            left = CstGuard::And(Box::new(left), Box::new(right), span);
        }
        Some(left)
    }

    pub(super) fn parse_guard_atom(&mut self) -> Option<CstGuard> {
        if matches!(self.peek(), Some(Token::LParen)) {
            self.bump();
            let inner = self.parse_guard_or()?;
            if !self.expect(&Token::RParen, "`)`") {
                return None;
            }
            return Some(inner);
        }
        let start_span = self.peek_span();
        let head = self.expect_ident()?;
        match head.name.as_str() {
            "metadata" => self.parse_guard_metadata(start_span),
            "iteration" => self.parse_guard_iteration(start_span, head.span.clone()),
            _ => {
                self.errors.push(
                    Diagnostic::error(
                        head.span.clone(),
                        format!(
                            "guard expressions must reference `metadata.<key>` or `iteration.<field>`, found `{}`",
                            head.name
                        ),
                    )
                    .with_hint(
                        "only `metadata.<key>` and `iteration.<field>` are permitted in `prompt when` guards",
                    ),
                );
                None
            }
        }
    }

    fn parse_guard_metadata(&mut self, start_span: Span) -> Option<CstGuard> {
        if !self.expect(&Token::Dot, "`.`") {
            return None;
        }
        let key = self.expect_ident()?;
        let op = self.bump()?.clone();
        let (s, _) = self.expect_string()?;
        let span = start_span.start..self.last_span().end;
        match op.token {
            Token::EqEq => Some(CstGuard::MetadataEq {
                key: key.name,
                value: s,
                span,
            }),
            Token::BangEq => Some(CstGuard::MetadataNeq {
                key: key.name,
                value: s,
                span,
            }),
            other @ (Token::LBrace
            | Token::RBrace
            | Token::LBracket
            | Token::RBracket
            | Token::LParen
            | Token::RParen
            | Token::Comma
            | Token::Equals
            | Token::Lt
            | Token::LtEq
            | Token::Gt
            | Token::GtEq
            | Token::Percent
            | Token::AmpAmp
            | Token::PipePipe
            | Token::Dot
            | Token::FatArrow
            | Token::String(_)
            | Token::Integer(_)
            | Token::Duration(_)
            | Token::True
            | Token::False
            | Token::Null
            | Token::Ident(_)) => {
                self.errors.push(Diagnostic::error(
                    op.span,
                    format!("expected `==` or `!=` in guard, found {}", other.describe()),
                ));
                None
            }
        }
    }

    fn parse_guard_iteration(&mut self, start_span: Span, _head_span: Span) -> Option<CstGuard> {
        if !self.expect(&Token::Dot, "`.`") {
            return None;
        }
        let field = self.expect_ident()?;
        let field_name = field.name.clone();
        let field_span = field.span.clone();

        // Optional `% N` reduction. Only legal on the LHS of a numeric
        // comparison; `previous_result %` is rejected at semantic time so
        // the parser captures the raw form here.
        let (modulus, modulus_span) = if matches!(self.peek(), Some(Token::Percent)) {
            self.bump();
            let modulus_tok = self.bump()?.clone();
            let m = match &modulus_tok.token {
                Token::Integer(n) => *n,
                other @ (Token::LBrace
                | Token::RBrace
                | Token::LBracket
                | Token::RBracket
                | Token::LParen
                | Token::RParen
                | Token::Comma
                | Token::Equals
                | Token::EqEq
                | Token::BangEq
                | Token::Lt
                | Token::LtEq
                | Token::Gt
                | Token::GtEq
                | Token::Percent
                | Token::AmpAmp
                | Token::PipePipe
                | Token::Dot
                | Token::FatArrow
                | Token::String(_)
                | Token::Duration(_)
                | Token::True
                | Token::False
                | Token::Null
                | Token::Ident(_)) => {
                    self.errors.push(Diagnostic::error(
                        modulus_tok.span.clone(),
                        format!(
                            "expected integer after `%` in iteration guard, found {}",
                            other.describe()
                        ),
                    ));
                    return None;
                }
            };
            (Some(m), Some(modulus_tok.span))
        } else {
            (None, None)
        };

        let op_tok = self.bump()?.clone();
        let op = match op_tok.token {
            Token::EqEq => CstCmpOp::Eq,
            Token::BangEq => CstCmpOp::Neq,
            Token::Lt => CstCmpOp::Lt,
            Token::LtEq => CstCmpOp::Le,
            Token::Gt => CstCmpOp::Gt,
            Token::GtEq => CstCmpOp::Ge,
            other @ (Token::LBrace
            | Token::RBrace
            | Token::LBracket
            | Token::RBracket
            | Token::LParen
            | Token::RParen
            | Token::Comma
            | Token::Equals
            | Token::Percent
            | Token::AmpAmp
            | Token::PipePipe
            | Token::Dot
            | Token::FatArrow
            | Token::String(_)
            | Token::Integer(_)
            | Token::Duration(_)
            | Token::True
            | Token::False
            | Token::Null
            | Token::Ident(_)) => {
                self.errors.push(Diagnostic::error(
                    op_tok.span.clone(),
                    format!(
                        "expected comparison operator (`==`, `!=`, `<`, `<=`, `>`, `>=`) in iteration guard, found {}",
                        other.describe()
                    ),
                ));
                return None;
            }
        };

        let rhs_tok = self.bump()?.clone();
        let span_end_inclusive = rhs_tok.span.end;
        let span = start_span.start..span_end_inclusive;
        match rhs_tok.token {
            Token::Integer(rhs) => Some(CstGuard::IterationCmp {
                field: field_name,
                field_span,
                modulus,
                modulus_span,
                op,
                op_span: op_tok.span,
                rhs,
                rhs_span: rhs_tok.span,
                span,
            }),
            Token::String(value) => self.parse_result_string_rhs(ResultStringRhs {
                field_name,
                field_span,
                modulus,
                op,
                value,
                rhs_span: rhs_tok.span,
                span,
            }),
            other @ (Token::LBrace
            | Token::RBrace
            | Token::LBracket
            | Token::RBracket
            | Token::LParen
            | Token::RParen
            | Token::Comma
            | Token::Equals
            | Token::EqEq
            | Token::BangEq
            | Token::Lt
            | Token::LtEq
            | Token::Gt
            | Token::GtEq
            | Token::Percent
            | Token::AmpAmp
            | Token::PipePipe
            | Token::Dot
            | Token::FatArrow
            | Token::Duration(_)
            | Token::True
            | Token::False
            | Token::Null
            | Token::Ident(_)) => {
                self.errors.push(Diagnostic::error(
                    rhs_tok.span.clone(),
                    format!(
                        "expected integer or string after iteration comparison operator, found {}",
                        other.describe()
                    ),
                ));
                None
            }
        }
    }

    fn parse_result_string_rhs(&mut self, args: ResultStringRhs) -> Option<CstGuard> {
        let ResultStringRhs {
            field_name,
            field_span,
            modulus,
            op,
            value,
            rhs_span,
            span,
        } = args;
        let result_equals = matches!(op, CstCmpOp::Eq) && modulus.is_none();
        let result_differs = matches!(op, CstCmpOp::Neq) && modulus.is_none();
        if !(result_equals || result_differs) {
            self.errors.push(
                Diagnostic::error(
                    rhs_span,
                    "string right-hand side is only valid for `iteration.previous_result == \"...\"` / `!= \"...\"`".to_string(),
                )
                .with_hint(
                    "`previous_result` only supports `==` / `!=` and cannot be reduced with `%`",
                ),
            );
            return None;
        }
        if result_equals {
            Some(CstGuard::IterationResultEq {
                field: field_name,
                field_span,
                value,
                value_span: rhs_span,
                span,
            })
        } else {
            Some(CstGuard::IterationResultNeq {
                field: field_name,
                field_span,
                value,
                value_span: rhs_span,
                span,
            })
        }
    }
}

struct ResultStringRhs {
    field_name: String,
    field_span: Span,
    modulus: Option<i64>,
    op: CstCmpOp,
    value: String,
    rhs_span: Span,
    span: Span,
}
