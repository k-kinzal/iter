//! Corpus-driven differential test: every `.iter` file shipped under
//! `tests/corpus/{valid,invalid}/` is fed to both parsers; they must agree
//! on accept/reject at the syntactic level, and the CST shape must match
//! (after span canonicalisation) on accepted inputs.
//!
//! Note: the corpus's `invalid/` files are predominantly *semantic*
//! failures — only `bad-string.iter` is a true syntax error. Semantic
//! rejects are accepted by both syntactic parsers; that is the expected
//! behavior and is asserted here explicitly.

use std::fs;
use std::path::{Path, PathBuf};

use iter_language::parse_to_cst;
use pretty_assertions::assert_eq;

use crate::oracle::{canonicalize, oracle_parse};

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
}

fn collect_iter_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("iter"))
        .collect();
    out.sort();
    out
}

fn handwritten_accepts(source: &str) -> bool {
    let (_cst, diagnostics) = parse_to_cst(source);
    !diagnostics
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error)
}

#[test]
fn valid_corpus_syntax_agreement() {
    let dir = corpus_root().join("valid");
    let files = collect_iter_files(&dir);
    assert!(!files.is_empty(), "no valid corpus files found in {dir:?}");

    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let hw_ok = handwritten_accepts(&source);
        let (oracle_cst, oracle_ok) = oracle_parse(&source);
        assert!(
            hw_ok && oracle_ok,
            "valid corpus file `{name}` must be accepted by both parsers (hand-written={hw_ok}, oracle={oracle_ok})",
        );

        let (hw_cst, _) = parse_to_cst(&source);
        let mut hw = hw_cst.expect("hand-written parser produced a CST");
        let mut oracle = oracle_cst.expect("oracle parser produced a CST");
        canonicalize(&mut hw);
        canonicalize(&mut oracle);
        assert_eq!(hw, oracle, "CST mismatch on valid corpus file `{name}`");
    }
}

#[test]
fn invalid_corpus_syntax_agreement() {
    let dir = corpus_root().join("invalid");
    let files = collect_iter_files(&dir);
    assert!(
        !files.is_empty(),
        "no invalid corpus files found in {dir:?}"
    );

    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let hw_ok = handwritten_accepts(&source);
        let (_oracle_cst, oracle_ok) = oracle_parse(&source);

        assert_eq!(
            hw_ok, oracle_ok,
            "invalid corpus file `{name}` must receive the same syntactic verdict (hand-written={hw_ok}, oracle={oracle_ok})",
        );
    }
}
