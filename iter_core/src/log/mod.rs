//! Generic log capture primitives — tagged byte streams, NDJSON
//! serialization, and a tailing reader.
//!
//! This module has no dependency on `crate::process`. Process-specific
//! wiring (policy, global sender, process-directory sinks) lives in
//! [`crate::process::log`].

mod reader;
mod sink;
mod stream;
mod writer;

pub use reader::{NdjsonReadError, NdjsonReader};
pub use sink::{NoopSink, OutputSink};
pub use stream::{LogEntry, LogStream};
pub(crate) use writer::{NdjsonWriter, WriterErrorSlot, WriterMsg, writer_dead_error};
