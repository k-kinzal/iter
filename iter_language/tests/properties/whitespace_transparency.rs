//! Whitespace transparency: the CST is invariant under changes to the
//! amount of whitespace between tokens (as long as the whitespace is still
//! legal — i.e. no newlines injected into a non-triple-quoted string and
//! no identifier-concatenation).

use iter_language::parse_to_cst;
use proptest::prelude::*;

use crate::oracle::{canonicalize, oracle_parse};

const CORPUS: &[&str] = &[
    "queue memory\nworkspace local { base = \".\" }\n",
    "agent claude { mode = print  command = \"claude\" }\n",
    "runner { continue_on_error = false }\nprompt \"body\"\n",
    "trigger loop { max_iteration = 5 delay = 10s }\n",
];

fn canon(src: &str) -> iter_language::RawFile {
    let (cst, _diag) = parse_to_cst(src);
    let mut cst = cst.expect("parser cst");
    canonicalize(&mut cst);
    cst
}

/// Replace every run of ASCII horizontal whitespace (` ` or `\t`) with a
/// random number of spaces drawn from `entropy` (1..=4). Newlines are
/// preserved so single-quoted strings remain legal.
fn perturb_whitespace(source: &str, entropy: &[u8]) -> String {
    let mut out = String::new();
    let mut iter = entropy.iter().copied();
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut in_triple = false;
    while let Some(c) = chars.next() {
        if in_triple {
            out.push(c);
            if c == '"' {
                // Check if this is the closing triple quote.
                let mut look = chars.clone();
                if look.next() == Some('"') && look.next() == Some('"') {
                    out.push('"');
                    out.push('"');
                    chars.next();
                    chars.next();
                    in_triple = false;
                }
            }
            continue;
        }
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        // Start of triple string?
        if c == '"' {
            let mut look = chars.clone();
            if look.next() == Some('"') && look.next() == Some('"') {
                out.push(c);
                out.push('"');
                out.push('"');
                chars.next();
                chars.next();
                in_triple = true;
                continue;
            }
            out.push(c);
            in_string = true;
            continue;
        }
        if c == ' ' || c == '\t' {
            // Collapse the run, then re-emit 1..=4 spaces based on entropy.
            while matches!(chars.peek(), Some(' ' | '\t')) {
                chars.next();
            }
            let n = (iter.next().unwrap_or(1) % 4) + 1;
            for _ in 0..n {
                out.push(' ');
            }
            continue;
        }
        out.push(c);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn whitespace_amount_is_transparent(
        corpus_idx in 0usize..CORPUS.len(),
        entropy in proptest::collection::vec(any::<u8>(), 1..32),
    ) {
        let base = CORPUS[corpus_idx];
        let baseline = canon(base);
        let perturbed_src = perturb_whitespace(base, &entropy);
        let perturbed = canon(&perturbed_src);
        prop_assert_eq!(
            &baseline,
            &perturbed,
            "whitespace perturbation changed CST:\n--- base ---\n{}\n--- perturbed ---\n{}",
            base,
            perturbed_src
        );
        let (oracle_cst, ok) = oracle_parse(&perturbed_src);
        prop_assert!(ok, "oracle rejected whitespace-perturbed input:\n{}", perturbed_src);
        let mut oracle = oracle_cst.unwrap();
        canonicalize(&mut oracle);
        prop_assert_eq!(&perturbed, &oracle, "hw/oracle disagreement under whitespace perturbation");
    }
}
