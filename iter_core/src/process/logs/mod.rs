//! Tailing reader over the per-process `log.ndjson` stream.
//!
//! `log.ndjson` is the unified, docker-logs-parity capture of every byte
//! the worker process emits — agent stdout, agent stderr, runner
//! tracing, lifecycle events. Each line is a JSON record:
//!
//! ```json
//! {"ts":"2026-05-03T10:40:07.980512Z","stream":"stderr","line":"starting runner ..."}
//! ```
//!
//! The writer side lives in [`crate::process::stdio::LogJsonSink`]; the
//! reader side lives in [`log_stream::LogStreamReader`] (this module's
//! sibling). The schema types ([`LogEntry`], [`LogStream`]) are shared
//! between writer and reader so the two ends always agree on field names
//! and ordering.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process::error::ProcessError;

mod log_stream;

pub use log_stream::LogStreamReader;

/// Polling interval used by the follow loop.
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Convenience wrapper used by the reader to map `io::Error` into
/// [`ProcessError::Io`] without an explicit `.map_err` closure at every
/// call site.
pub(super) fn io_err(e: std::io::Error) -> ProcessError {
    ProcessError::Io(e)
}

/// Which originating stream (stdout vs stderr) a single
/// [`LogEntry`] came from. Serialised as `"stdout"` / `"stderr"` so the
/// on-disk format matches the docker `json-file` driver convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    /// The worker's stdout — typically the agent's intentional output.
    Stdout,
    /// The worker's stderr — diagnostics: agent stderr, runner tracing,
    /// lifecycle INFO events.
    Stderr,
}

/// One record in the per-process `log.ndjson` stream.
///
/// `line` is the original text with any trailing `\n` (or `\r\n`)
/// stripped — NDJSON itself uses `\n` as the record separator, so the
/// payload string never needs to carry one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// UTC timestamp (RFC 3339, sub-second precision) when the entry
    /// was enqueued by the writer.
    pub ts: DateTime<Utc>,
    /// Which stream emitted this line.
    pub stream: LogStream,
    /// The line itself, without the trailing newline.
    pub line: String,
}
