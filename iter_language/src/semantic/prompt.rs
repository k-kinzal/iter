//! Expression lowering for runner prompt-match arms.

use super::{Analyzer, lower_expr_pure};
use crate::ast::Expr;
use crate::parser::CstExpr;

impl Analyzer {
    pub(super) fn lower_prompt_expr(&mut self, expression: CstExpr) -> Expr {
        lower_expr_pure(
            expression,
            &mut self.errors,
            &["signal", "metadata", "iteration", "var"],
        )
    }
}
