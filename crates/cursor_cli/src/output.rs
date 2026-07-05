//! Typed model of `cursor-agent --print`'s `json` / `stream-json` output.
//!
//! `--output-format json` emits a single terminal `result` object;
//! `--output-format stream-json` emits a newline-delimited event stream that
//! ends with the same `result` record. [`PrintOutput::parse`] accepts both:
//! it first tries to read the whole stream as one JSON document, and otherwise
//! collects each JSON-object line as an [`Event`].
//!
//! Each event is preserved losslessly and exposed through typed accessors.
//! [`PrintOutput`] derives the terminal verdict — session id, request id,
//! final message, and usage — from the last `result` record, and surfaces a
//! `type: "error"` record's message for failure diagnostics.
//!
//! # The `is_error` field
//!
//! The terminal `result` record carries an `is_error` boolean that is
//! hard-coded `false` in this CLI revision, so it carries no information. The
//! success signal is the *presence* of the terminal `result` record; callers
//! should not treat `is_error` as authoritative. It is exposed via
//! [`PrintOutput::is_error_flag`] only for completeness.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::cli::Error;

/// One record from `cursor-agent --print`, preserved losslessly.
///
/// The underlying JSON object is retained verbatim so no field is lost across
/// CLI versions; typed accessors read individual fields defensively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    raw: Value,
}

impl Event {
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

    /// Classified event type, read from the `type` field.
    #[must_use]
    pub fn event_type(&self) -> EventType {
        EventType::from_marker(self.type_marker())
    }

    fn type_marker(&self) -> Option<&str> {
        self.raw
            .get("type")
            .or_else(|| self.raw.get("kind"))
            .and_then(Value::as_str)
    }

    /// Does this event carry the given `type`?
    fn type_is(&self, marker: &str) -> bool {
        self.type_marker() == Some(marker)
    }

    /// Read a string field defensively.
    fn string_field(&self, key: &str) -> Option<String> {
        self.raw.get(key).and_then(Value::as_str).map(str::to_owned)
    }
}

impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Value::deserialize(deserializer)?;
        if raw.is_object() {
            Ok(Self { raw })
        } else {
            Err(D::Error::custom("cursor-agent event must be a JSON object"))
        }
    }
}

/// Known `cursor-agent --print` record types.
///
/// Only the terminal `result` record and the `error` record are verified
/// against the pinned CLI version; the `stream-json` format's intermediate
/// event vocabulary (assistant / tool / system records) is recognized when
/// present but is not exhaustively pinned, so any other `type` falls through
/// to [`Other`].
///
/// [`Other`]: EventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `result` — the terminal record of a completed run.
    Result,
    /// `error` — an error record emitted in place of a `result`.
    Error,
    /// `system` — a system/init record.
    System,
    /// `user` — an echoed user turn.
    User,
    /// `assistant` — an assistant message record.
    Assistant,
    /// `tool_call` — a tool invocation record.
    ToolCall,
    /// A record whose `type` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("result") => Self::Result,
            Some("error") => Self::Error,
            Some("system") => Self::System,
            Some("user") => Self::User,
            Some("assistant") => Self::Assistant,
            Some("tool_call") => Self::ToolCall,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// Token usage reported in the terminal `result` record's `usage` object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// `usage.input_tokens`, when reported.
    pub input_tokens: Option<u64>,
    /// `usage.output_tokens`, when reported.
    pub output_tokens: Option<u64>,
    /// `usage.num_turns`, when reported.
    pub num_turns: Option<u64>,
}

impl Usage {
    fn from_value(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        Self {
            input_tokens: value.get("input_tokens").and_then(Value::as_u64),
            output_tokens: value.get("output_tokens").and_then(Value::as_u64),
            num_turns: value.get("num_turns").and_then(Value::as_u64),
        }
    }
}

/// Parsed `cursor-agent --print` output: the record log plus derived accessors
/// for the terminal verdict.
///
/// Non-JSON and non-object lines are skipped, mirroring how a streaming
/// revision may interleave machine-readable records with incidental text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrintOutput {
    events: Vec<Event>,
}

impl PrintOutput {
    /// Parse a `cursor-agent --print` stdout stream.
    ///
    /// The `json` format is a single JSON document, so the whole (trimmed)
    /// stream is tried first; failing that, the `stream-json` format is parsed
    /// leniently line-by-line, with every JSON-object line becoming an
    /// [`Event`] and any other line ignored.
    #[must_use]
    pub fn parse(stdout: &str) -> Self {
        if let Some(value) = single_object(stdout) {
            return Self {
                events: vec![Event::from_value(value)],
            };
        }
        let events = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(Value::is_object)
            .map(Event::from_value)
            .collect();
        Self { events }
    }

    /// The parsed records, in stream order.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The terminal `result` record, when the stream produced one.
    ///
    /// This is the success signal: its presence means `cursor-agent` completed
    /// a turn. Later records win when a stream carries more than one.
    #[must_use]
    pub fn result_record(&self) -> Option<&Event> {
        self.events
            .iter()
            .rev()
            .find(|event| event.type_is("result"))
    }

    /// The last `type: "error"` record, when one was emitted.
    #[must_use]
    pub fn error_record(&self) -> Option<&Event> {
        self.events.iter().rev().find(|event| event.type_is("error"))
    }

    /// `true` when a terminal `result` record is present.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.result_record().is_some()
    }

    /// `session_id` from the terminal `result` record.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.result_record()
            .and_then(|event| event.string_field("session_id"))
    }

    /// `request_id` from the terminal `result` record.
    #[must_use]
    pub fn request_id(&self) -> Option<String> {
        self.result_record()
            .and_then(|event| event.string_field("request_id"))
    }

    /// Final assistant message (`result`) from the terminal record.
    #[must_use]
    pub fn final_message(&self) -> Option<String> {
        self.result_record()
            .and_then(|event| event.string_field("result"))
    }

    /// `subtype` from the terminal `result` record (e.g. `success`).
    #[must_use]
    pub fn subtype(&self) -> Option<String> {
        self.result_record()
            .and_then(|event| event.string_field("subtype"))
    }

    /// The terminal record's hard-coded `is_error` flag.
    ///
    /// This is `false` in the pinned CLI revision regardless of outcome and
    /// carries no information; see the [module docs](self). Exposed only for
    /// completeness — do not use it as a success/failure signal.
    #[must_use]
    pub fn is_error_flag(&self) -> Option<bool> {
        self.result_record()
            .and_then(|event| event.as_value().get("is_error"))
            .and_then(Value::as_bool)
    }

    /// `duration_ms` from the terminal `result` record.
    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.result_record()
            .and_then(|event| event.as_value().get("duration_ms"))
            .and_then(Value::as_u64)
    }

    /// Parsed `usage` object from the terminal `result` record.
    #[must_use]
    pub fn usage(&self) -> Usage {
        Usage::from_value(
            self.result_record()
                .and_then(|event| event.as_value().get("usage")),
        )
    }

    /// A short failure diagnostic from a `type: "error"` record's `message`
    /// (or `error`) field, when one was emitted.
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        let event = self.error_record()?;
        event
            .string_field("message")
            .or_else(|| event.string_field("error"))
    }
}

impl TryFrom<ProcessOutput> for PrintOutput {
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

/// Parse `stdout` as a single JSON object (the `--output-format json`
/// contract: the whole stream is one JSON document). Returns `None` when the
/// stream is empty, is not valid JSON, or is not a JSON object.
fn single_object(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) if value.is_object() => Some(value),
        Ok(_) | Err(_) => None,
    }
}

/// A live or collected stream of `cursor-agent --print --output-format
/// stream-json` events.
///
/// This type represents both live process output and already-collected output.
/// Callers consume it through the standard [`Stream`] trait.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event, Error>> + Send>>,
}

impl EventStream {
    pub(crate) fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Event, Error>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Create a stream from already-collected events.
    #[must_use]
    pub fn from_events(events: Vec<Event>) -> Self {
        Self::from_stream(stream::iter(events.into_iter().map(Ok)))
    }
}

impl fmt::Debug for EventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl Stream for EventStream {
    type Item = Result<Event, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}
