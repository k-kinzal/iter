//! Duration normalisation: units `s`, `m`, `h`, `d` normalise to seconds
//! with the canonical conversion factors (1m = 60s, 1h = 3600s, 1d =
//! 86400s). Equivalent durations expressed in different units must
//! produce equal `CstValue::Duration` values in the CST.

use iter_language::{CstSection, CstValue, parse_to_cst};
use pretty_assertions::assert_eq;

use crate::oracle::{canonicalize, oracle_parse};

fn duration_field(source: &str) -> i64 {
    let (cst, _) = parse_to_cst(source);
    let file = cst.expect("cst");
    let body = match &file.sections[0] {
        CstSection::Block { body, .. } => body.as_ref().expect("body"),
        CstSection::Prompt { .. } | CstSection::On { .. } => {
            panic!("expected block section")
        }
    };
    match &body.fields[0].value {
        CstValue::Duration(secs, _) => *secs,
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::Ident(..)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected duration, got {other:?}"),
    }
}

fn assert_parsers_agree(source: &str) {
    let (hw_cst, _) = parse_to_cst(source);
    let (oracle_cst, ok) = oracle_parse(source);
    assert!(ok, "oracle rejected duration input:\n{source}");
    let mut hw = hw_cst.expect("hw cst");
    let mut oracle = oracle_cst.expect("oracle cst");
    canonicalize(&mut hw);
    canonicalize(&mut oracle);
    assert_eq!(hw, oracle, "hw vs oracle disagreement on:\n{source}");
}

#[test]
fn seconds_is_canonical() {
    assert_eq!(duration_field("trigger loop { delay = 3600s }\n"), 3600);
    assert_parsers_agree("trigger loop { delay = 3600s }\n");
}

#[test]
fn minutes_multiply_by_sixty() {
    assert_eq!(duration_field("trigger loop { delay = 60m }\n"), 3600);
    assert_parsers_agree("trigger loop { delay = 60m }\n");
}

#[test]
fn hours_multiply_by_3600() {
    assert_eq!(duration_field("trigger loop { delay = 1h }\n"), 3600);
    assert_parsers_agree("trigger loop { delay = 1h }\n");
}

#[test]
fn days_multiply_by_86400() {
    assert_eq!(duration_field("trigger loop { delay = 1d }\n"), 86400);
    assert_parsers_agree("trigger loop { delay = 1d }\n");
}

#[test]
fn one_hour_equals_3600_seconds() {
    let a = duration_field("trigger loop { delay = 1h }\n");
    let b = duration_field("trigger loop { delay = 3600s }\n");
    assert_eq!(a, b);
}

#[test]
fn one_day_equals_24_hours() {
    let a = duration_field("trigger loop { delay = 1d }\n");
    let b = duration_field("trigger loop { delay = 24h }\n");
    assert_eq!(a, b);
}
