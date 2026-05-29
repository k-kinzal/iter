//! Generic [`Value`] used inside [`super::TriggerDecl::External`] and other
//! field bags.

use std::collections::BTreeMap;

/// Generic value used inside [`super::TriggerDecl::External`] and other field bags.
///
/// This is a side AST that mirrors the surface field grammar (strings,
/// integers, booleans, durations, identifiers, lists, blocks, function calls).
/// It exists so the language crate can preserve user-defined trigger payloads
/// without dragging in `serde_json` and without losing fidelity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// String literal value.
    String(String),
    /// Integer literal value.
    Integer(i64),
    /// Boolean literal value.
    Bool(bool),
    /// `null` literal — the explicit absence of a value.
    Null,
    /// Duration value, normalised to seconds.
    DurationSecs(i64),
    /// Bareword identifier value (e.g. `print`, `interactive`, `normal`).
    Ident(String),
    /// Homogeneous or heterogeneous list of values.
    List(Vec<Value>),
    /// Nested `{ ... }` block represented as ordered key/value pairs.
    Block(BTreeMap<String, Value>),
    /// Function call such as `env("VAR")` or `regex("...")`.
    Call {
        /// Function name.
        name: String,
        /// Argument values in declaration order.
        args: Vec<Value>,
    },
}
