//! Longest-match property: multi-character operators (`==`, `!=`, `&&`,
//! `||`) must tokenise as single tokens. If the lexer split `==` into two
//! `=` tokens, a `prompt when metadata.x == "y"` expression would fail to
//! parse as a guard comparison.
//!
//! The check works by taking each operator and exercising it in its only
//! legal context (guard expressions). Both parsers must accept the input;
//! the hand-written parser's accept means the operator was not split.

use iter_language::parse_to_cst;

use crate::oracle::oracle_parse;

fn hw_ok(source: &str) -> bool {
    let (_cst, diag) = parse_to_cst(source);
    !diag
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error)
}

fn both_accept(source: &str) {
    let hw = hw_ok(source);
    let (_, o) = oracle_parse(source);
    assert!(
        hw && o,
        "expected both parsers to accept:\n{source}\n(hw={hw}, oracle={o})"
    );
}

#[test]
fn eqeq_is_one_token() {
    both_accept("prompt when metadata.k == \"v\" \"body\"");
}

#[test]
fn bangeq_is_one_token() {
    both_accept("prompt when metadata.k != \"v\" \"body\"");
}

#[test]
fn ampamp_is_one_token() {
    both_accept("prompt when metadata.k == \"v\" && metadata.k != \"w\" \"body\"");
}

#[test]
fn pipepipe_is_one_token() {
    both_accept("prompt when metadata.k == \"v\" || metadata.k == \"w\" \"body\"");
}

#[test]
fn eqeq_with_tight_spacing() {
    // Adjacent `==` without surrounding whitespace is still a single token.
    both_accept("prompt when metadata.k==\"v\" \"body\"");
}

#[test]
fn bangeq_with_tight_spacing() {
    both_accept("prompt when metadata.k!=\"v\" \"body\"");
}

#[test]
fn single_equals_is_not_operator_in_guard() {
    // `=` (assignment) must not be accepted in guard position. A `=` in a
    // guard would parse only if the lexer produced a single `=` where the
    // grammar required `==`/`!=`; both parsers must reject this shape.
    let src = "prompt when metadata.k = \"v\" \"body\"";
    let hw = hw_ok(src);
    let (_, o) = oracle_parse(src);
    assert!(
        !hw && !o,
        "expected both parsers to reject a bare `=` in guard: hw={hw}, oracle={o}"
    );
}
