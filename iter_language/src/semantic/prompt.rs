//! The small `lower_guard` shim shared by runner prompt-match arms.

use super::{Analyzer, lower_guard_pure};
use crate::ast::PromptGuard;
use crate::parser::CstGuard;

impl Analyzer {
    pub(super) fn lower_guard(&mut self, guard: CstGuard) -> PromptGuard {
        lower_guard_pure(guard, &mut self.errors)
    }
}
