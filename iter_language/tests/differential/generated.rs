//! Generated-input differential tests: build a random `CstFile` with
//! `proptest`, pretty-print it to source with the test-only pretty printer,
//! then re-parse the result with both the hand-written parser and the
//! pest-based oracle; require that they accept and produce structurally
//! identical CSTs (after span canonicalisation).
//!
//! The default `#[test]` runs a small 64-case budget so the test stays
//! quick in routine `cargo test`; the `#[ignore]`d companion raises that
//! to 4096 for nightly soak runs (`cargo test -- --ignored`).

use iter_language::parse_to_cst;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use crate::oracle::{canonicalize, oracle_parse, pretty, strategy::file_strategy};

fn assert_round_trips(mut original: iter_language::CstFile) {
    let src = pretty(&original);

    let (hw_cst, hw_diag) = parse_to_cst(&src);
    let hw_ok = !hw_diag
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error);
    assert!(
        hw_ok,
        "hand-written parser rejected pretty output:\n--- src ---\n{src}\n--- diag ---\n{hw_diag:?}"
    );

    let (oracle_cst, oracle_ok) = oracle_parse(&src);
    assert!(
        oracle_ok,
        "oracle parser rejected pretty output:\n--- src ---\n{src}"
    );

    let mut hw = hw_cst.expect("hand-written parser returned a CST");
    let mut oracle = oracle_cst.expect("oracle parser returned a CST");
    canonicalize(&mut hw);
    canonicalize(&mut oracle);
    canonicalize(&mut original);

    assert_eq!(
        hw, oracle,
        "CST mismatch between parsers on generated input:\n{src}"
    );
    assert_eq!(
        hw, original,
        "round-trip CST drift — pretty-printed output re-parses to a different shape:\n{src}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn generated_files_round_trip(file in file_strategy()) {
        assert_round_trips(file);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    #[ignore = "heavy: 4096 cases; enable with --ignored for soak runs"]
    fn generated_files_round_trip_heavy(file in file_strategy()) {
        assert_round_trips(file);
    }
}
