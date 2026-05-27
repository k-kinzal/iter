//! Recursive-descent parser producing a Concrete Syntax Tree (CST).
//!
//! The CST is intentionally generic: each top-level section is captured as a
//! `RawSection { keyword, kind, fields/body }` tuple. Lowering to the public
//! [`crate::ast::Root`] is performed by [`crate::semantic`], which is
//! also where field validation lives. Splitting parsing from lowering keeps
//! grammar concerns and "domain dispatch" concerns separate, which is
//! exactly what makes the AST a stable contract.
//!
//! # Stability
//!
//! The CST types (`RawFile`, `RawSection`, `RawBlock`, `RawField`, `RawValue`,
//! `RawIdent`, `RawRoute`, `RawAction`, `RawGuard`) are part of the public
//! grammar contract together with [`crate::GRAMMAR_VERSION`]. The
//! [`crate::parse_to_cst`] entry point returns them directly so that external
//! tooling — in particular the oracle-parser differential harness in this
//! crate's test suite — can reason about the syntactic layer without pulling
//! in semantic validation.

mod cst;
mod cursor;
mod guard;
mod prompt;
mod section;
mod value;

pub use cst::{
    RawAction, RawBlock, RawCmpOp, RawEventHandler, RawField, RawFile, RawGuard, RawIdent,
    RawPromptMatchArm, RawRoute, RawSection, RawValue,
};

use crate::diagnostic::Diagnostic;
use crate::lexer::SpannedToken;

pub(crate) fn parse_tokens(
    tokens: &[SpannedToken],
    source_len: usize,
) -> (Option<RawFile>, Vec<Diagnostic>) {
    let mut parser = Parser {
        tokens,
        pos: 0,
        source_len,
        errors: Vec::new(),
    };
    let file = parser.parse_file();
    (Some(file), parser.errors)
}

struct Parser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    source_len: usize,
    errors: Vec<Diagnostic>,
}
