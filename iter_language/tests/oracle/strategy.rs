//! `proptest` strategies for generating `CstFile` CSTs.
//!
//! The generated trees are intentionally constrained so that the
//! test-only pretty printer in `pretty.rs` can render them back to source
//! without loss. Specifically:
//!   * identifiers are drawn from a bareword alphabet and never equal a
//!     contextual block-entry keyword (`on`, `shell`) when placed as a
//!     field name, since the formal grammar rejects such fields,
//!   * booleans and `null` are emitted as `CstValue::Bool`/`CstValue::Null`,
//!     never as `CstValue::Ident` with name `"true"`/`"false"`/`"null"`, to
//!     stay on the side of the grammar that prefers `boolean`/`null` before
//!     `ident` in `value`,
//!   * strings contain only characters the pretty printer can round-trip,
//!   * `List` values are non-nested above a small bound to keep the search
//!     space manageable.
//!
//! Canonicalised `Span(0..0)` is used everywhere; callers that compare CST
//! shape should run `canonicalize::canonicalize` on both sides anyway.

use iter_language::{
    CstAction, CstBlock, CstField, CstFile, CstGuard, CstIdent, CstRoute, CstSection, CstValue,
};
use proptest::collection::vec;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Basic tokens
// ---------------------------------------------------------------------------

/// Lowercase bareword identifier of length 1..=8. Never equal to a keyword
/// that would be consumed as something other than an identifier by the
/// containing context.
fn ident_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_filter("not a reserved word in ident position", |s| {
        !matches!(
            s.as_str(),
            "true" | "false" | "null" | "on" | "shell" | "when" | "metadata"
        )
    })
}

/// A kind identifier for `queue`/`workspace`/`agent`/`trigger` sections.
/// Uses the same alphabet as [`ident_name`].
fn kind_name() -> impl Strategy<Value = String> {
    ident_name()
}

/// One of the event-handler event names. Limited to the canonical set so
/// the semantic analyzer never rejects them, but that is not required for
/// the CST-only differential tests — we constrain purely for readability.
fn event_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("runner_starting".to_string()),
        Just("signal_received".to_string()),
        Just("workspace_setup_starting".to_string()),
        Just("workspace_setup_finished".to_string()),
        Just("agent_starting".to_string()),
        Just("agent_finished".to_string()),
        Just("workspace_teardown_starting".to_string()),
        Just("workspace_teardown_finished".to_string()),
        Just("runner_error".to_string()),
        Just("runner_finished".to_string()),
    ]
}

/// A string literal. Only characters the pretty printer can round-trip.
fn string_lit() -> impl Strategy<Value = String> {
    // Printable ASCII except control chars, with occasional escape triggers.
    // We exclude `\x00..=\x1F` because the pretty printer would `\u{..}`
    // them and the round trip is still safe, but keeping them out makes
    // failure messages easier to read.
    "[ -~]{0,16}".prop_map(|s| s)
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

fn value_leaf() -> impl Strategy<Value = CstValue> {
    prop_oneof![
        string_lit().prop_map(|s| CstValue::String(s, 0..0)),
        (0i64..1_000_000).prop_map(|n| CstValue::Integer(n, 0..0)),
        (1i64..10_000).prop_map(|n| CstValue::Duration(n, 0..0)),
        any::<bool>().prop_map(|b| CstValue::Bool(b, 0..0)),
        Just(CstValue::Null(0..0)),
        ident_name().prop_map(|s| CstValue::Ident(s, 0..0)),
    ]
}

fn value_strategy() -> impl Strategy<Value = CstValue> {
    value_leaf().prop_recursive(
        2,  // up to 2 levels of recursion
        16, // max total size
        4,  // items per collection
        |inner| {
            prop_oneof![
                vec(inner.clone(), 0..=3).prop_map(|items| CstValue::List(items, 0..0)),
                (ident_name(), vec(inner.clone(), 0..=2)).prop_map(|(name, args)| CstValue::Call {
                    name,
                    args,
                    span: 0..0,
                }),
                // Block-as-value: at most one level of nested fields.
                vec((ident_name(), inner), 0..=3).prop_map(|entries| {
                    let fields = entries
                        .into_iter()
                        .map(|(name, value)| CstField {
                            name: CstIdent { name, span: 0..0 },
                            value,
                            span: 0..0,
                        })
                        .collect();
                    CstValue::Block(CstBlock {
                        fields,
                        routes: Vec::new(),
                        actions: Vec::new(),
                        prompt_arms: Vec::new(),
                        event_handlers: Vec::new(),
                        span: 0..0,
                    })
                }),
            ]
        },
    )
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

fn field_strategy() -> impl Strategy<Value = CstField> {
    (ident_name(), value_strategy()).prop_map(|(name, value)| CstField {
        name: CstIdent { name, span: 0..0 },
        value,
        span: 0..0,
    })
}

fn action_strategy() -> impl Strategy<Value = CstAction> {
    string_lit().prop_map(|command| CstAction {
        keyword_span: 0..0,
        command,
        span: 0..0,
    })
}

fn route_strategy() -> impl Strategy<Value = CstRoute> {
    (
        string_lit(),
        proptest::option::of(string_lit()),
        vec(field_strategy(), 0..=3),
    )
        .prop_map(|(event_pattern, when, fields)| CstRoute {
            event_pattern,
            when_span: when.as_ref().map(|_| 0..0),
            when,
            body: CstBlock {
                fields,
                routes: Vec::new(),
                actions: Vec::new(),
                prompt_arms: Vec::new(),
                event_handlers: Vec::new(),
                span: 0..0,
            },
            span: 0..0,
        })
}

fn block_strategy() -> impl Strategy<Value = CstBlock> {
    (
        vec(field_strategy(), 0..=4),
        vec(route_strategy(), 0..=2),
        vec(action_strategy(), 0..=2),
    )
        .prop_map(|(fields, routes, actions)| CstBlock {
            fields,
            routes,
            actions,
            prompt_arms: Vec::new(),
            event_handlers: Vec::new(),
            span: 0..0,
        })
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

fn guard_leaf() -> impl Strategy<Value = CstGuard> {
    (ident_name(), string_lit(), any::<bool>()).prop_map(|(key, value, eq)| {
        if eq {
            CstGuard::MetadataEq {
                key,
                value,
                span: 0..0,
            }
        } else {
            CstGuard::MetadataNeq {
                key,
                value,
                span: 0..0,
            }
        }
    })
}

fn guard_strategy() -> impl Strategy<Value = CstGuard> {
    guard_leaf().prop_recursive(2, 8, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(l, r)| CstGuard::And(
                Box::new(l),
                Box::new(r),
                0..0
            )),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| CstGuard::Or(
                Box::new(l),
                Box::new(r),
                0..0
            )),
        ]
    })
}

fn block_section_strategy() -> impl Strategy<Value = CstSection> {
    (
        prop_oneof![
            Just("queue".to_string()),
            Just("workspace".to_string()),
            Just("agent".to_string()),
            Just("trigger".to_string()),
        ],
        kind_name(),
        proptest::option::of(block_strategy()),
    )
        .prop_map(|(keyword, kind, body)| CstSection::Block {
            keyword,
            keyword_span: 0..0,
            kind: Some(CstIdent {
                name: kind,
                span: 0..0,
            }),
            kind2: None,
            alias: None,
            body,
            span: 0..0,
        })
}

fn runner_section_strategy() -> impl Strategy<Value = CstSection> {
    proptest::option::of(block_strategy()).prop_map(|body| CstSection::Block {
        keyword: "runner".into(),
        keyword_span: 0..0,
        kind: None,
        kind2: None,
        alias: None,
        body,
        span: 0..0,
    })
}

fn prompt_section_strategy() -> impl Strategy<Value = CstSection> {
    (proptest::option::of(guard_strategy()), string_lit()).prop_map(|(guard, body)| {
        CstSection::Prompt {
            keyword_span: 0..0,
            name: None,
            guard,
            body,
            body_span: 0..0,
            span: 0..0,
        }
    })
}

fn on_section_strategy() -> impl Strategy<Value = CstSection> {
    (event_name(), block_strategy()).prop_map(|(event, body)| CstSection::On {
        keyword_span: 0..0,
        event: CstIdent {
            name: event,
            span: 0..0,
        },
        body,
        span: 0..0,
    })
}

pub(crate) fn section_strategy() -> impl Strategy<Value = CstSection> {
    prop_oneof![
        block_section_strategy(),
        runner_section_strategy(),
        prompt_section_strategy(),
        on_section_strategy(),
    ]
}

pub(crate) fn file_strategy() -> impl Strategy<Value = CstFile> {
    vec(section_strategy(), 0..=5).prop_map(|sections| CstFile { sections })
}
