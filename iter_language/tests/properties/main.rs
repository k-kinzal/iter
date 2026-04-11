//! Lexer/parser property tests: assertions about the surface syntax that
//! the grammar file alone cannot encode (longest-match, keyword vs. ident
//! boundary, whitespace/comment transparency, duration normalisation,
//! idempotence). Every test runs both the hand-written implementation and
//! the pest-based oracle and requires them to agree.

#[path = "../oracle/mod.rs"]
mod oracle;

mod comment_transparency;
mod duration_normalization;
mod idempotence;
mod keyword_ident;
mod longest_match;
mod whitespace_transparency;
