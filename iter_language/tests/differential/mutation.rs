//! Mutation differential tests: start from each corpus file, perturb it
//! with a deterministic byte-level mutator driven by `arbitrary`, and
//! verify that the hand-written and oracle parsers reach the same
//! accept/reject verdict on the mutated input.
//!
//! We deliberately do *not* require CST-shape equality on mutated inputs:
//! error recovery in the hand-written parser produces a best-effort CST on
//! rejected inputs, whereas pest returns no CST at all. Accept/reject
//! agreement is the minimum guarantee that catches divergent "tokenises
//! differently in the face of noise" bugs.

use std::fs;
use std::path::{Path, PathBuf};

use arbitrary::{Arbitrary, Unstructured};
use iter_language::parse_to_cst;
use proptest::prelude::*;

use crate::oracle::oracle_parse;

fn corpus_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus");
    let mut out = Vec::new();
    for subdir in ["valid", "invalid"] {
        let dir: PathBuf = root.join(subdir);
        for entry in
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        {
            let p = entry.unwrap().path();
            if p.extension().and_then(|s| s.to_str()) != Some("iter") {
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let source = fs::read_to_string(&p).unwrap();
            out.push((format!("{subdir}/{name}"), source));
        }
    }
    out.sort();
    out
}

fn handwritten_accepts(source: &str) -> bool {
    let (_cst, diagnostics) = parse_to_cst(source);
    !diagnostics
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error)
}

/// Apply a sequence of byte-level edits to `base` driven by the raw entropy
/// in `bytes`. Each edit either inserts, deletes, or swaps a single byte at
/// a random offset. We re-establish UTF-8 by replacing any byte that would
/// leave the string invalid, so the mutated input stays a legal `&str`.
fn mutate(base: &str, bytes: &[u8]) -> String {
    let mut buf: Vec<u8> = base.as_bytes().to_vec();
    let mut u = Unstructured::new(bytes);
    // Cap the number of edits so a single fuzz case doesn't chase a runaway
    // budget; `arbitrary` already handles "ran out of bytes" gracefully.
    for _ in 0..8 {
        if u.is_empty() {
            break;
        }
        let op = u8::arbitrary(&mut u).unwrap_or(0) % 3;
        let len = buf.len().max(1);
        let idx = usize::arbitrary(&mut u).unwrap_or(0) % len;
        match op {
            0 => {
                // Insert a printable ASCII byte.
                let c = (u8::arbitrary(&mut u).unwrap_or(b' ') % 0x5F).saturating_add(0x20);
                buf.insert(idx.min(buf.len()), c);
            }
            1 => {
                if !buf.is_empty() {
                    buf.remove(idx);
                }
            }
            _ => {
                if !buf.is_empty() {
                    let c = (u8::arbitrary(&mut u).unwrap_or(b' ') % 0x5F).saturating_add(0x20);
                    buf[idx] = c;
                }
            }
        }
    }
    String::from_utf8(buf).unwrap_or_else(|e| {
        // Replace invalid bytes with `?` so we always return a legal &str.
        String::from_utf8_lossy(&e.into_bytes())
            .replace(char::REPLACEMENT_CHARACTER, "?")
            .to_string()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn mutated_corpus_verdicts_agree(
        seed in 0usize..1000,
        entropy in proptest::collection::vec(any::<u8>(), 0..128),
    ) {
        let corpus = corpus_files();
        let (name, base) = &corpus[seed % corpus.len()];
        let mutated = mutate(base, &entropy);

        let hw_ok = handwritten_accepts(&mutated);
        let (_, oracle_ok) = oracle_parse(&mutated);

        prop_assert_eq!(
            hw_ok,
            oracle_ok,
            "accept/reject divergence on mutation of {}:\n--- input ---\n{}",
            name,
            mutated
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    #[ignore = "heavy: 4096 cases; enable with --ignored for soak runs"]
    fn mutated_corpus_verdicts_agree_heavy(
        seed in 0usize..1000,
        entropy in proptest::collection::vec(any::<u8>(), 0..128),
    ) {
        let corpus = corpus_files();
        let (name, base) = &corpus[seed % corpus.len()];
        let mutated = mutate(base, &entropy);

        let hw_ok = handwritten_accepts(&mutated);
        let (_, oracle_ok) = oracle_parse(&mutated);

        prop_assert_eq!(
            hw_ok,
            oracle_ok,
            "accept/reject divergence on mutation of {}:\n--- input ---\n{}",
            name,
            mutated
        );
    }
}
