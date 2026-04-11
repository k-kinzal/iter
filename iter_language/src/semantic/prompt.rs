//! `prompt [when ...] "..."` section lowering plus the small `lower_guard` shim.

use super::{Analyzer, lower_guard_pure};
use crate::ast::{PromptDecl, PromptGuard, Span, Spanned};
use crate::parser::RawGuard;

impl Analyzer {
    pub(super) fn lower_prompt(
        &mut self,
        guard: Option<RawGuard>,
        body: String,
        span: Span,
        body_span: Span,
    ) -> Spanned<PromptDecl> {
        self.validate_template(&body, &body_span);
        let guard = guard.map(|g| self.lower_guard(g));
        Spanned::new(PromptDecl { guard, body }, span)
    }

    pub(super) fn lower_guard(&mut self, guard: RawGuard) -> PromptGuard {
        lower_guard_pure(guard, &mut self.errors)
    }
}
