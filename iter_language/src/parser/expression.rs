//! Common expression recursive descent.

use super::Parser;
use super::cst::{CstBinaryOp, CstExpr, CstExprLiteral, CstPathSegment};
use crate::diagnostic::Diagnostic;
use crate::lexer::Token;

impl Parser<'_> {
    pub(super) fn parse_expr(&mut self) -> Option<CstExpr> {
        self.parse_expr_or()
    }

    fn parse_expr_or(&mut self) -> Option<CstExpr> {
        let mut left = self.parse_expr_and()?;
        while matches!(self.peek(), Some(Token::PipePipe)) {
            let op_span = self.peek_span();
            self.bump();
            let right = self.parse_expr_and()?;
            let span = left.span().start..right.span().end;
            left = CstExpr::Binary {
                lhs: Box::new(left),
                op: CstBinaryOp::Or,
                op_span,
                rhs: Box::new(right),
                span,
            };
        }
        Some(left)
    }

    fn parse_expr_and(&mut self) -> Option<CstExpr> {
        let mut left = self.parse_expr_comparison()?;
        while matches!(self.peek(), Some(Token::AmpAmp)) {
            let op_span = self.peek_span();
            self.bump();
            let right = self.parse_expr_comparison()?;
            let span = left.span().start..right.span().end;
            left = CstExpr::Binary {
                lhs: Box::new(left),
                op: CstBinaryOp::And,
                op_span,
                rhs: Box::new(right),
                span,
            };
        }
        Some(left)
    }

    fn parse_expr_comparison(&mut self) -> Option<CstExpr> {
        let left = self.parse_expr_modulus()?;
        let Some(op) = self.peek().and_then(comparison_op) else {
            return Some(left);
        };
        let op_span = self.peek_span();
        self.bump();
        let right = self.parse_expr_modulus()?;
        let span = left.span().start..right.span().end;
        Some(CstExpr::Binary {
            lhs: Box::new(left),
            op,
            op_span,
            rhs: Box::new(right),
            span,
        })
    }

    fn parse_expr_modulus(&mut self) -> Option<CstExpr> {
        let mut left = self.parse_expr_primary()?;
        while matches!(self.peek(), Some(Token::Percent)) {
            let op_span = self.peek_span();
            self.bump();
            let right = self.parse_expr_primary()?;
            let span = left.span().start..right.span().end;
            left = CstExpr::Binary {
                lhs: Box::new(left),
                op: CstBinaryOp::Mod,
                op_span,
                rhs: Box::new(right),
                span,
            };
        }
        Some(left)
    }

    fn parse_expr_primary(&mut self) -> Option<CstExpr> {
        let token = self.tokens.get(self.pos)?.clone();
        match token.token {
            Token::LParen => {
                self.bump();
                let expression = self.parse_expr_or()?;
                if !self.expect(&Token::RParen, "`)`") {
                    return None;
                }
                Some(expression)
            }
            Token::String(value) => {
                self.bump();
                Some(CstExpr::Literal {
                    value: CstExprLiteral::String(value),
                    span: token.span,
                })
            }
            Token::Integer(value) => {
                self.bump();
                Some(CstExpr::Literal {
                    value: CstExprLiteral::Integer(value),
                    span: token.span,
                })
            }
            Token::True => {
                self.bump();
                Some(CstExpr::Literal {
                    value: CstExprLiteral::Bool(true),
                    span: token.span,
                })
            }
            Token::False => {
                self.bump();
                Some(CstExpr::Literal {
                    value: CstExprLiteral::Bool(false),
                    span: token.span,
                })
            }
            Token::Null => {
                self.bump();
                Some(CstExpr::Literal {
                    value: CstExprLiteral::Null,
                    span: token.span,
                })
            }
            Token::Ident(_) => self.parse_expr_path(),
            other => {
                self.errors.push(Diagnostic::error(
                    token.span,
                    format!("expected expression, found {}", other.describe()),
                ));
                None
            }
        }
    }

    fn parse_expr_path(&mut self) -> Option<CstExpr> {
        let root = self.expect_ident()?;
        let start = root.span.start;
        let mut end = root.span.end;
        let mut segments = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Dot) => {
                    self.bump();
                    let field = self.expect_ident()?;
                    end = field.span.end;
                    segments.push(CstPathSegment::Field(field));
                }
                Some(Token::LBracket) => {
                    self.bump();
                    let index_token = self.tokens.get(self.pos)?.clone();
                    let Token::Integer(index) = index_token.token else {
                        self.errors.push(Diagnostic::error(
                            index_token.span,
                            "expression array index must be a non-negative integer",
                        ));
                        return None;
                    };
                    self.bump();
                    if !self.expect(&Token::RBracket, "`]`") {
                        return None;
                    }
                    let Ok(index) = usize::try_from(index) else {
                        self.errors.push(Diagnostic::error(
                            index_token.span,
                            array_index_overflow_message(index),
                        ));
                        return None;
                    };
                    end = self.last_span().end;
                    segments.push(CstPathSegment::Index(index, index_token.span));
                }
                _ => break,
            }
        }
        Some(CstExpr::Path {
            root,
            segments,
            span: start..end,
        })
    }
}

fn array_index_overflow_message(index: i64) -> String {
    format!(
        "expression array index `{index}` exceeds this platform's maximum supported index `{}`",
        usize::MAX
    )
}

fn comparison_op(token: &Token) -> Option<CstBinaryOp> {
    match token {
        Token::EqEq => Some(CstBinaryOp::Eq),
        Token::BangEq => Some(CstBinaryOp::Neq),
        Token::Lt => Some(CstBinaryOp::Lt),
        Token::LtEq => Some(CstBinaryOp::Le),
        Token::Gt => Some(CstBinaryOp::Gt),
        Token::GtEq => Some(CstBinaryOp::Ge),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::array_index_overflow_message;

    #[test]
    fn array_index_overflow_diagnostic_names_value_and_platform_limit() {
        let message = array_index_overflow_message(i64::MAX);
        assert!(message.contains(&i64::MAX.to_string()));
        assert!(message.contains(&usize::MAX.to_string()));
        assert!(message.contains("maximum supported index"));
    }
}
