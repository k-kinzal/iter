//! Schema types shared between the NDJSON writer and reader.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
