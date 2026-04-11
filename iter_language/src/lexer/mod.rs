//! Lexer for the iter workflow definition language.
//!
//! Converts a source string into a vector of [`SpannedToken`]s. Errors are
//! collected and returned alongside the tokens; the lexer never aborts on
//! the first malformed token.

mod scanner;
mod token;

pub(crate) use scanner::lex;
pub(crate) use token::{SpannedToken, Token};
