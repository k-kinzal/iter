//! Comment transparency: `# ...\n` is skipped between tokens and does not
//! affect the CST. Proptest takes a corpus file plus a set of (offset,
//! comment text) pairs, inserts the comments at whitespace boundaries, and
//! asserts the hand-written parser's CST is unchanged.
//!
//! Span canonicalisation is mandatory — the inserted comments of course
//! shift source offsets.

use iter_language::parse_to_cst;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use crate::oracle::{canonicalize, oracle_parse};

const CORPUS: &[&str] = &[
    // A representative slice — the interesting lexer state lives in the
    // tokens themselves, not in the section structure, so we keep this
    // short.
    "queue memory\nworkspace local { base = \".\" }\n",
    "runner { continue_on_error = false }\nprompt \"body\"\n",
    "agent claude {\n  mode = print\n  command = \"claude\"\n}\n",
    "trigger loop { max_iteration = 5 }\n",
];

fn canon(src: &str) -> iter_language::RawFile {
    let (cst, _diag) = parse_to_cst(src);
    let mut cst = cst.expect("parser produced a cst");
    canonicalize(&mut cst);
    cst
}

fn insert_comments(source: &str, at: &[usize], texts: &[String]) -> String {
    let mut out = String::new();
    // Only insert at positions that are newline terminators so we don't
    // mid-token-splice a comment. We use `split_inclusive('\n')` to
    // preserve the line breaks.
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if at.iter().any(|&a| a % lines.len().max(1) == i) {
            let text = &texts[i % texts.len().max(1)];
            out.push_str("# ");
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn inserted_comments_are_transparent(
        corpus_idx in 0usize..CORPUS.len(),
        at in proptest::collection::vec(0usize..16, 0..6),
        texts in proptest::collection::vec("[a-zA-Z0-9 _]{0,20}", 1..4),
    ) {
        let base = CORPUS[corpus_idx];
        let baseline = canon(base);
        let perturbed_src = insert_comments(base, &at, &texts);
        let perturbed = canon(&perturbed_src);
        prop_assert_eq!(
            &baseline,
            &perturbed,
            "comment insertion changed CST shape:\n--- base ---\n{}\n--- perturbed ---\n{}",
            base,
            perturbed_src
        );
        // And the oracle must agree.
        let (oracle_cst, ok) = oracle_parse(&perturbed_src);
        prop_assert!(ok, "oracle rejected input with comments:\n{}", perturbed_src);
        let mut oracle = oracle_cst.unwrap();
        canonicalize(&mut oracle);
        prop_assert_eq!(&perturbed, &oracle, "hw/oracle disagreement on commented input");
    }
}

#[test]
fn trailing_comment_without_newline() {
    // A comment at end of file without a trailing newline is still consumed
    // by both parsers.
    let src = "queue memory # trailing, no newline";
    let hw = canon(src);
    let (oracle_cst, ok) = oracle_parse(src);
    assert!(ok, "oracle rejected trailing-comment input");
    let mut oracle = oracle_cst.unwrap();
    canonicalize(&mut oracle);
    assert_eq!(hw, oracle);
}
