//! Keyword vs identifier boundary: a reserved word followed by an
//! identifier-continuation character must tokenise as a single bareword
//! identifier rather than as a keyword followed by a remainder.
//!
//! For example, `truest` must be one identifier, not `true` plus `st`.
//! Likewise for `false`, `on`, `shell`, `when`, `metadata`, and every
//! block keyword.

use iter_language::{CstSection, CstValue, parse_to_cst};

use crate::oracle::{canonicalize, oracle_parse};

fn parse_both(source: &str) -> (iter_language::CstFile, iter_language::CstFile) {
    let (hw_cst, _) = parse_to_cst(source);
    let (oracle_cst, oracle_ok) = oracle_parse(source);
    assert!(oracle_ok, "oracle rejected input:\n{source}");
    let mut hw = hw_cst.expect("hw cst");
    let mut oracle = oracle_cst.expect("oracle cst");
    canonicalize(&mut hw);
    canonicalize(&mut oracle);
    assert_eq!(hw, oracle, "hw vs oracle disagreed on:\n{source}");
    (hw, oracle)
}

#[test]
fn true_with_suffix_is_ident() {
    // As a field VALUE.
    let (hw, _) = parse_both("queue memory\nworkspace local { base = truest }\n");
    // Navigate: second section is workspace, its body has field base = ident "truest".
    let section = &hw.sections[1];
    let body = if let CstSection::Block { body, .. } = section {
        body.as_ref().unwrap()
    } else {
        panic!()
    };
    match &body.fields[0].value {
        CstValue::Ident(name, _) => assert_eq!(name, "truest"),
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected ident truest, got {other:?}"),
    }
}

#[test]
fn false_with_suffix_is_ident() {
    let (hw, _) = parse_both("workspace local { base = falsehood }\n");
    let body = if let CstSection::Block { body, .. } = &hw.sections[0] {
        body.as_ref().unwrap()
    } else {
        panic!()
    };
    match &body.fields[0].value {
        CstValue::Ident(name, _) => assert_eq!(name, "falsehood"),
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected ident falsehood, got {other:?}"),
    }
}

#[test]
fn null_with_suffix_is_ident() {
    let (hw, _) = parse_both("workspace local { base = nullish }\n");
    let body = if let CstSection::Block { body, .. } = &hw.sections[0] {
        body.as_ref().unwrap()
    } else {
        panic!()
    };
    match &body.fields[0].value {
        CstValue::Ident(name, _) => assert_eq!(name, "nullish"),
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected ident nullish, got {other:?}"),
    }
}

#[test]
fn exact_null_is_null() {
    let (hw, _) = parse_both("workspace local { base = null }\n");
    let body = if let CstSection::Block { body, .. } = &hw.sections[0] {
        body.as_ref().unwrap()
    } else {
        panic!()
    };
    match &body.fields[0].value {
        CstValue::Null(_) => {}
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Ident(..)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected null, got {other:?}"),
    }
}

#[test]
fn exact_true_is_bool() {
    let (hw, _) = parse_both("workspace local { preserve_mtime = true }\n");
    let body = if let CstSection::Block { body, .. } = &hw.sections[0] {
        body.as_ref().unwrap()
    } else {
        panic!()
    };
    match &body.fields[0].value {
        CstValue::Bool(true, _) => {}
        other @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::Ident(..)
        | CstValue::List(..)
        | CstValue::Block(_)
        | CstValue::Call { .. }) => panic!("expected bool true, got {other:?}"),
    }
}

#[test]
fn on_with_suffix_is_field_name() {
    // `online_mode` must be a field name, not `on` + `line_mode`.
    parse_both("runner { online_mode = true }\n");
}

#[test]
fn when_with_suffix_is_ident_in_prompt() {
    // `whenever` as a prompt suffix word; `prompt whenever ...` should not be
    // parsed as `prompt when ever ...`. Instead it would parse as a section
    // starting with `prompt`, expected ident `whenever` → but prompt expects
    // a string next, so both parsers reject.
    let src = "prompt whenever \"body\"";
    let hw = !parse_to_cst(src)
        .1
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error);
    let (_, o) = oracle_parse(src);
    assert_eq!(hw, o, "hw vs oracle verdict differs on:\n{src}");
}

#[test]
fn queue_with_suffix_is_ident() {
    // `queueish` is a single bareword identifier, not `queue` + `ish`.
    // At top level it is not a known section keyword → both parsers reject.
    let src = "queueish memory\n";
    let hw = !parse_to_cst(src)
        .1
        .iter()
        .any(|d| d.severity == iter_language::Severity::Error);
    let (_, o) = oracle_parse(src);
    assert_eq!(
        hw, o,
        "hw vs oracle verdict differs on `queueish memory`: hw={hw}, oracle={o}"
    );
}
