//! Idempotence: parsing the same input twice produces the same CST.
//! Trivially necessary for the rest of the test suite to mean anything,
//! but worth a property test anyway so a hypothetical future regression
//! (e.g. a thread-local cache that mutates across calls) is caught.

use iter_language::parse_to_cst;
use proptest::prelude::*;

use crate::oracle::{canonicalize, oracle_parse, pretty, strategy::file_strategy};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn hand_written_is_idempotent(file in file_strategy()) {
        let src = pretty(&file);
        let a = parse_to_cst(&src).0.expect("cst a");
        let b = parse_to_cst(&src).0.expect("cst b");
        prop_assert_eq!(a, b, "hand-written parser returned different CSTs for the same input");
    }

    #[test]
    fn oracle_is_idempotent(file in file_strategy()) {
        let src = pretty(&file);
        let (a, _) = oracle_parse(&src);
        let (b, _) = oracle_parse(&src);
        let (a, b) = (a.expect("oracle a"), b.expect("oracle b"));
        prop_assert_eq!(a, b, "oracle parser returned different CSTs for the same input");
    }

    #[test]
    fn parsing_twice_equals_parsing_once_then_canonicalising(file in file_strategy()) {
        let src = pretty(&file);
        let mut once = parse_to_cst(&src).0.expect("cst");
        canonicalize(&mut once);
        let mut twice = parse_to_cst(&src).0.expect("cst");
        canonicalize(&mut twice);
        prop_assert_eq!(once, twice);
    }
}
