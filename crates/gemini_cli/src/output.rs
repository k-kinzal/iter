//! Typed models of Gemini's machine-readable output.
//!
//! Gemini exposes two machine-readable modes:
//!
//! * `gemini -o json` emits a **single** terminal JSON object —
//!   `{ "response": <text>, "stats": { "tokens": {...} }, "error": {...} }`.
//!   [`GeminiOutput`] wraps that object losslessly and exposes typed
//!   accessors for the fields Gemini 0.41.2 documents.
//! * `gemini -o stream-json` emits a newline-delimited event stream.
//!   [`StreamEvent`] preserves each line verbatim and [`StreamOutput`] collects
//!   them and derives the terminal verdict by field presence. Gemini 0.41.2
//!   does not publish its stream-json event vocabulary through `--help`, so an
//!   event's `type`/`kind` marker is classified conservatively:
//!   [`StreamEventType::Other`] carries any marker the crate does not model.
//!
//! Deciding what a non-zero exit, an `error` field, or a usage limit *means*
//! for a consumer is deliberately left to the caller — this crate reports the
//! CLI's own output faithfully and nothing more.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde_json::Value;

use crate::cli::Error;

/// Token statistics reported under `stats.tokens` in a terminal record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenStats {
    /// Input / prompt tokens.
    pub input: Option<u64>,
    /// Output / completion tokens.
    pub output: Option<u64>,
    /// Total tokens.
    pub total: Option<u64>,
}

impl TokenStats {
    /// Read a `stats.tokens` object out of a value's payload.
    fn from_payload(payload: &Value) -> Self {
        let tokens = payload.pointer("/stats/tokens");
        let read = |key: &str| {
            tokens
                .and_then(|tokens| tokens.get(key))
                .and_then(Value::as_u64)
        };
        Self {
            input: read("input"),
            output: read("output"),
            total: read("total"),
        }
    }
}

/// The `error` object Gemini attaches to a failing terminal record.
///
/// The mere presence of an `error` field signals failure; the fields refine
/// *why*. All are optional because Gemini does not guarantee every one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultError {
    /// `error.type`, when present.
    pub error_type: Option<String>,
    /// `error.message`, when present.
    pub message: Option<String>,
    /// `error.code`, when present.
    pub code: Option<i32>,
}

impl ResultError {
    fn from_value(error: &Value) -> Self {
        Self {
            error_type: error.get("type").and_then(Value::as_str).map(str::to_owned),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
            code: error.get("code").and_then(Value::as_i64).map(|c| c as i32),
        }
    }
}

/// A parsed `gemini -o json` terminal record, preserved losslessly.
///
/// The raw JSON value is retained so no field is lost across Gemini versions;
/// typed accessors read the documented fields on top of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiOutput {
    raw: Value,
}

impl GeminiOutput {
    /// Parse a `gemini -o json` stdout stream as its single JSON value.
    ///
    /// Returns `None` when the stream is empty or is not valid JSON — the
    /// signal that Gemini never produced a terminal record. Any valid JSON
    /// value (object or otherwise) is retained; the typed accessors read
    /// through it and simply report `None` for a non-object shape.
    #[must_use]
    pub fn parse(stdout: &str) -> Option<Self> {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return None;
        }
        let raw = serde_json::from_str::<Value>(trimmed).ok()?;
        Some(Self { raw })
    }

    /// An empty record, standing in for a successful run that produced no JSON.
    #[must_use]
    pub fn empty() -> Self {
        Self { raw: Value::Null }
    }

    /// Wrap a raw JSON value as a terminal record.
    #[must_use]
    pub fn from_value(raw: Value) -> Self {
        Self { raw }
    }

    /// Borrow the raw JSON value.
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.raw
    }

    /// Return the raw JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.raw
    }

    /// Final assistant message (`response`).
    #[must_use]
    pub fn response(&self) -> Option<String> {
        self.raw
            .get("response")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Session / conversation id, when the record carries one.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.raw
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Token statistics from `stats.tokens`, when reported.
    #[must_use]
    pub fn tokens(&self) -> TokenStats {
        TokenStats::from_payload(&self.raw)
    }

    /// The `error` object, when the record carries a non-null `error` field.
    #[must_use]
    pub fn error(&self) -> Option<ResultError> {
        let error = self.raw.get("error").filter(|value| !value.is_null())?;
        Some(ResultError::from_value(error))
    }

    /// Did the record carry an `error` field?
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.error().is_some()
    }
}

impl Default for GeminiOutput {
    fn default() -> Self {
        Self::empty()
    }
}

impl TryFrom<ProcessOutput> for GeminiOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        match Self::parse(&stdout) {
            Some(record) => Ok(record),
            None => {
                if output.status.success() {
                    Ok(Self::empty())
                } else {
                    Err(Error::Cli {
                        exit_code: output.status.code(),
                        stdout: stdout.into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            }
        }
    }
}

/// One line from `gemini -o stream-json`, preserved losslessly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    raw: Value,
}

impl StreamEvent {
    /// Wrap a raw JSON value as an event.
    #[must_use]
    pub fn from_value(raw: Value) -> Self {
        Self { raw }
    }

    /// Borrow the raw JSON value.
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.raw
    }

    /// Return the raw JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.raw
    }

    /// The event's `type`/`kind` marker, when present.
    #[must_use]
    pub fn marker(&self) -> Option<&str> {
        self.raw
            .get("type")
            .or_else(|| self.raw.get("kind"))
            .and_then(Value::as_str)
    }

    /// Classified event type. Any marker the crate does not model — including
    /// an absent one — maps to [`StreamEventType::Other`].
    #[must_use]
    pub fn event_type(&self) -> StreamEventType {
        StreamEventType::from_marker(self.marker())
    }

    fn string_field(&self, key: &str) -> Option<String> {
        self.raw.get(key).and_then(Value::as_str).map(str::to_owned)
    }
}

/// Classified `gemini -o stream-json` event type.
///
/// Gemini 0.41.2 does not enumerate its stream-json event vocabulary through
/// `--help`, so this enum intentionally carries a single [`Other`] catch-all
/// that preserves the raw marker. It is `#[non_exhaustive]`: once Gemini
/// documents concrete event types, they can be added without a breaking change.
///
/// [`Other`]: StreamEventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamEventType {
    /// An event whose `type`/`kind` marker was absent or unrecognized.
    Other(Option<String>),
}

impl StreamEventType {
    fn from_marker(marker: Option<&str>) -> Self {
        // Gemini's stream-json markers are undocumented in 0.41.2; every
        // marker is preserved verbatim until a schema is published.
        Self::Other(marker.map(str::to_owned))
    }
}

/// Parsed `gemini -o stream-json` output: the event log plus derived accessors
/// for the terminal verdict.
///
/// Non-JSON and non-object lines are skipped, mirroring how Gemini may
/// interleave the machine-readable events with incidental text. The verdict
/// accessors work by field presence, which is robust to the (undocumented)
/// event `type` markers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamOutput {
    events: Vec<StreamEvent>,
}

impl StreamOutput {
    /// Parse a `gemini -o stream-json` stdout stream leniently: every
    /// JSON-object line becomes a [`StreamEvent`]; any other line is ignored.
    #[must_use]
    pub fn parse(stdout: &str) -> Self {
        let events = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(Value::is_object)
            .map(StreamEvent::from_value)
            .collect();
        Self { events }
    }

    /// The parsed events, in stream order.
    #[must_use]
    pub fn events(&self) -> &[StreamEvent] {
        &self.events
    }

    fn last_matching<F>(&self, predicate: F) -> Option<&StreamEvent>
    where
        F: Fn(&StreamEvent) -> bool,
    {
        self.events.iter().rev().find(|event| predicate(event))
    }

    /// Final assistant message, from the latest event exposing a `response`.
    #[must_use]
    pub fn response(&self) -> Option<String> {
        self.last_matching(|event| event.string_field("response").is_some())
            .and_then(|event| event.string_field("response"))
    }

    /// Session id, from the latest event exposing one.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.last_matching(|event| event.string_field("session_id").is_some())
            .and_then(|event| event.string_field("session_id"))
    }

    /// Token statistics, from the latest event carrying a `stats.tokens` block.
    #[must_use]
    pub fn tokens(&self) -> TokenStats {
        self.last_matching(|event| event.as_value().pointer("/stats/tokens").is_some())
            .map(|event| TokenStats::from_payload(event.as_value()))
            .unwrap_or_default()
    }

    /// The `error` object, from the latest event carrying a non-null `error`.
    #[must_use]
    pub fn error(&self) -> Option<ResultError> {
        self.last_matching(|event| {
            event
                .as_value()
                .get("error")
                .is_some_and(|value| !value.is_null())
        })
        .and_then(|event| event.as_value().get("error").map(ResultError::from_value))
    }
}

impl TryFrom<ProcessOutput> for StreamOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = Self::parse(&stdout);
        if parsed.events.is_empty() && !output.status.success() {
            return Err(Error::Cli {
                exit_code: output.status.code(),
                stdout: stdout.into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(parsed)
    }
}

/// A live or collected stream of `gemini -o stream-json` events.
///
/// This type represents both live process output and already-collected output.
/// Callers consume it through the standard [`Stream`] trait.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>>,
}

impl EventStream {
    pub(crate) fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<StreamEvent, Error>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Create a stream from already-collected events.
    #[must_use]
    pub fn from_events(events: Vec<StreamEvent>) -> Self {
        Self::from_stream(stream::iter(events.into_iter().map(Ok)))
    }
}

impl fmt::Debug for EventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl Stream for EventStream {
    type Item = Result<StreamEvent, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}
