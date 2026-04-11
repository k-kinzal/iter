//! Differential test binary: assert that the pest-based oracle parser and
//! the hand-written implementation agree on accept/reject and on CST shape
//! over three input sources — the corpus, proptest-generated `RawFile`s
//! pretty-printed back to source, and arbitrary byte mutations of the
//! corpus.

#[path = "../oracle/mod.rs"]
mod oracle;

mod corpus;
mod generated;
mod mutation;
