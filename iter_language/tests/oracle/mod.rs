//! Shared oracle-parser helpers used by the `differential` and `properties`
//! integration test binaries.
//!
//! This module is included via `#[path = "../oracle/mod.rs"] mod oracle;` from
//! each test binary's `main.rs`; it is intentionally not a crate of its own.
//!
//! Responsibilities:
//!   * expose a `pest`-driven second parser for the iter language
//!     (`parser.rs`),
//!   * lower the oracle's `Pairs` tree into the *same* `RawFile` CST shape
//!     that the hand-written parser produces (`lowering.rs`),
//!   * canonicalize spans so structural comparison is span-oblivious
//!     (`canonicalize.rs`),
//!   * render a `RawFile` back to source for round-trip and generated-input
//!     tests (`pretty.rs`), and
//!   * supply `proptest` / `arbitrary` generation strategies
//!     (`strategy.rs`).

pub(crate) mod canonicalize;
pub(crate) mod lowering;
pub(crate) mod parser;
pub(crate) mod pretty;
pub(crate) mod strategy;

pub(crate) use canonicalize::canonicalize;
pub(crate) use parser::oracle_parse;
pub(crate) use pretty::pretty;
