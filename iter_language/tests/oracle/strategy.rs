//! `proptest` strategies for generating `RawFile` CSTs.
//!
//! The generated trees are intentionally constrained so that the
//! test-only pretty printer in `pretty.rs` can render them back to source
//! without loss. Specifically:
//!   * identifiers are drawn from a bareword alphabet and never equal a
//!     contextual block-entry keyword (`on`, `shell`) when placed as a
//!     field name, since the formal grammar rejects such fields,
//!   * booleans and `null` are emitted as `RawValue::Bool`/`RawValue::Null`,
//!     never as `RawValue::Ident` with name `"true"`/`"false"`/`"null"`, to
//!     stay on the side of the grammar that prefers `boolean`/`null` before
//!     `ident` in `value`,
//!   * strings contain only characters the pretty printer can round-trip,
//!   * `List` values are non-nested above a small bound to keep the search
//!     space manageable.
//!
//! Canonicalised `Span(0..0)` is used everywhere; callers that compare CST
//! shape should run `canonicalize::canonicalize` on both sides anyway.

use iter_language::{
    RawAction, RawBlock, RawField, RawFile, RawGuard, RawIdent, RawRoute, RawSection, RawValue,
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

fn value_leaf() -> impl Strategy<Value = RawValue> {
    prop_oneof![
        string_lit().prop_map(|s| RawValue::String(s, 0..0)),
        (0i64..1_000_000).prop_map(|n| RawValue::Integer(n, 0..0)),
        (1i64..10_000).prop_map(|n| RawValue::Duration(n, 0..0)),
        any::<bool>().prop_map(|b| RawValue::Bool(b, 0..0)),
        Just(RawValue::Null(0..0)),
        ident_name().prop_map(|s| RawValue::Ident(s, 0..0)),
    ]
}

fn value_strategy() -> impl Strategy<Value = RawValue> {
    value_leaf().prop_recursive(
        2,  // up to 2 levels of recursion
        16, // max total size
        4,  // items per collection
        |inner| {
            prop_oneof![
                vec(inner.clone(), 0..=3).prop_map(|items| RawValue::List(items, 0..0)),
                (ident_name(), vec(inner.clone(), 0..=2)).prop_map(|(name, args)| RawValue::Call {
                    name,
                    args,
                    span: 0..0,
                }),
                // Block-as-value: at most one level of nested fields.
                vec((ident_name(), inner), 0..=3).prop_map(|entries| {
                    let fields = entries
                        .into_iter()
                        .map(|(name, value)| RawField {
                            name: RawIdent { name, span: 0..0 },
                            value,
                            span: 0..0,
                        })
                        .collect();
                    RawValue::Block(RawBlock {
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

fn field_strategy() -> impl Strategy<Value = RawField> {
    (ident_name(), value_strategy()).prop_map(|(name, value)| RawField {
        name: RawIdent { name, span: 0..0 },
        value,
        span: 0..0,
    })
}

fn action_strategy() -> impl Strategy<Value = RawAction> {
    string_lit().prop_map(|command| RawAction {
        keyword_span: 0..0,
        command,
        span: 0..0,
    })
}

fn route_strategy() -> impl Strategy<Value = RawRoute> {
    (
        string_lit(),
        proptest::option::of(string_lit()),
        vec(field_strategy(), 0..=3),
    )
        .prop_map(|(event_pattern, when, fields)| RawRoute {
            event_pattern,
            when,
            body: RawBlock {
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

fn block_strategy() -> impl Strategy<Value = RawBlock> {
    (
        vec(field_strategy(), 0..=4),
        vec(route_strategy(), 0..=2),
        vec(action_strategy(), 0..=2),
    )
        .prop_map(|(fields, routes, actions)| RawBlock {
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

fn guard_leaf() -> impl Strategy<Value = RawGuard> {
    (ident_name(), string_lit(), any::<bool>()).prop_map(|(key, value, eq)| {
        if eq {
            RawGuard::MetadataEq {
                key,
                value,
                span: 0..0,
            }
        } else {
            RawGuard::MetadataNeq {
                key,
                value,
                span: 0..0,
            }
        }
    })
}

fn guard_strategy() -> impl Strategy<Value = RawGuard> {
    guard_leaf().prop_recursive(2, 8, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(l, r)| RawGuard::And(
                Box::new(l),
                Box::new(r),
                0..0
            )),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| RawGuard::Or(
                Box::new(l),
                Box::new(r),
                0..0
            )),
        ]
    })
}

fn block_section_strategy() -> impl Strategy<Value = RawSection> {
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
        .prop_map(|(keyword, kind, body)| RawSection::Block {
            keyword,
            keyword_span: 0..0,
            kind: Some(RawIdent {
                name: kind,
                span: 0..0,
            }),
            kind2: None,
            alias: None,
            body,
            span: 0..0,
        })
}

fn runner_section_strategy() -> impl Strategy<Value = RawSection> {
    proptest::option::of(block_strategy()).prop_map(|body| RawSection::Block {
        keyword: "runner".into(),
        keyword_span: 0..0,
        kind: None,
        kind2: None,
        alias: None,
        body,
        span: 0..0,
    })
}

fn prompt_section_strategy() -> impl Strategy<Value = RawSection> {
    (proptest::option::of(guard_strategy()), string_lit()).prop_map(|(guard, body)| {
        RawSection::Prompt {
            keyword_span: 0..0,
            name: None,
            guard,
            body,
            body_span: 0..0,
            span: 0..0,
        }
    })
}

fn on_section_strategy() -> impl Strategy<Value = RawSection> {
    (event_name(), block_strategy()).prop_map(|(event, body)| RawSection::On {
        keyword_span: 0..0,
        event: RawIdent {
            name: event,
            span: 0..0,
        },
        body,
        span: 0..0,
    })
}

pub(crate) fn section_strategy() -> impl Strategy<Value = RawSection> {
    prop_oneof![
        block_section_strategy(),
        runner_section_strategy(),
        prompt_section_strategy(),
        on_section_strategy(),
    ]
}

pub(crate) fn file_strategy() -> impl Strategy<Value = RawFile> {
    vec(section_strategy(), 0..=5).prop_map(|sections| RawFile { sections })
}
