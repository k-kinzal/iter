//! Conformance test runner.
//!
//! Walks every `.iter` file under `tests/corpus/{valid,invalid}/`. Files in
//! `valid/` must parse successfully and have a sibling `<file>.iter.ast.snap`
//! describing the resulting [`iter_language::Iterfile`] AST. Files in
//! `invalid/` must fail and have a sibling `<file>.iter.err.snap` containing
//! the rendered diagnostic output produced by [`iter_language::Diagnostic::report`].
//!
//! Snapshots live next to their source files so a third party can ship the
//! corpus as the canonical conformance test suite.

use std::fs;
use std::path::{Path, PathBuf};

use iter_language::{Diagnostic, parse, parse_compose};

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

fn render_diagnostics(name: &str, source: &str, diags: &[Diagnostic]) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&d.report(name, source));
        out.push('\n');
    }
    out
}

#[test]
fn valid_corpus() {
    let dir = corpus_root().join("valid");
    let files = collect_iter_files(&dir);
    assert!(!files.is_empty(), "no valid corpus files found in {dir:?}");
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let snapshot_name = format!("{name}.ast");
        match parse(&source) {
            Ok(root) => {
                insta::with_settings!(
                    {
                        snapshot_path => "corpus/valid",
                        prepend_module_to_snapshot => false,
                        description => name.clone(),
                        omit_expression => true,
                    },
                    {
                        insta::assert_debug_snapshot!(snapshot_name, root);
                    }
                );
            }
            Err(diags) => {
                let rendered = render_diagnostics(&name, &source, &diags);
                panic!("expected `{name}` to parse successfully, got:\n{rendered}");
            }
        }
    }
}

#[test]
fn invalid_corpus() {
    let dir = corpus_root().join("invalid");
    let files = collect_iter_files(&dir);
    assert!(
        !files.is_empty(),
        "no invalid corpus files found in {dir:?}"
    );
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let snapshot_name = format!("{name}.err");
        match parse(&source) {
            Ok(_) => panic!("expected `{name}` to fail parsing, but it succeeded"),
            Err(diags) => {
                let rendered = render_diagnostics(&name, &source, &diags);
                insta::with_settings!(
                    {
                        snapshot_path => "corpus/invalid",
                        prepend_module_to_snapshot => false,
                        description => name.clone(),
                        omit_expression => true,
                    },
                    {
                        insta::assert_snapshot!(snapshot_name, rendered);
                    }
                );
            }
        }
    }
}

#[test]
fn valid_compose_corpus() {
    let dir = corpus_root().join("compose").join("valid");
    if !dir.exists() {
        return;
    }
    let files = collect_iter_files(&dir);
    assert!(
        !files.is_empty(),
        "no valid compose corpus files found in {dir:?}"
    );
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let snapshot_name = format!("{name}.ast");
        match parse_compose(&source) {
            Ok(root) => {
                insta::with_settings!(
                    {
                        snapshot_path => "corpus/compose/valid",
                        prepend_module_to_snapshot => false,
                        description => name.clone(),
                        omit_expression => true,
                    },
                    {
                        insta::assert_debug_snapshot!(snapshot_name, root);
                    }
                );
            }
            Err(diags) => {
                let rendered = render_diagnostics(&name, &source, &diags);
                panic!("expected compose `{name}` to parse successfully, got:\n{rendered}");
            }
        }
    }
}

#[test]
fn invalid_compose_corpus() {
    let dir = corpus_root().join("compose").join("invalid");
    if !dir.exists() {
        return;
    }
    let files = collect_iter_files(&dir);
    assert!(
        !files.is_empty(),
        "no invalid compose corpus files found in {dir:?}"
    );
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let snapshot_name = format!("{name}.err");
        match parse_compose(&source) {
            Ok(_) => panic!("expected compose `{name}` to fail parsing, but it succeeded"),
            Err(diags) => {
                let rendered = render_diagnostics(&name, &source, &diags);
                insta::with_settings!(
                    {
                        snapshot_path => "corpus/compose/invalid",
                        prepend_module_to_snapshot => false,
                        description => name.clone(),
                        omit_expression => true,
                    },
                    {
                        insta::assert_snapshot!(snapshot_name, rendered);
                    }
                );
            }
        }
    }
}

#[test]
fn grammar_version_is_semver_like() {
    let v = iter_language::GRAMMAR_VERSION;
    let parts: Vec<&str> = v.split('.').collect();
    assert_eq!(parts.len(), 3, "GRAMMAR_VERSION must be MAJOR.MINOR.PATCH");
    for p in parts {
        p.parse::<u32>()
            .unwrap_or_else(|_| panic!("non-numeric component in GRAMMAR_VERSION: {v}"));
    }
}
