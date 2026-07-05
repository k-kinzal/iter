//! Typed model of `cline --json`'s NDJSON run stream.
//!
//! `cline --json` streams newline-delimited JSON records to stdout: any number
//! of progress / error events followed by a terminal `run_result` record.
//! [`Event`] preserves each line losslessly and exposes typed accessors;
//! [`RunOutput`] collects the stream and derives the terminal verdict.
//!
//! # Records this crate keys off
//!
//! ```jsonc
//! { "type": "run_result", "finishReason": "completed", "sessionId": "<id>",
//!   "message": "<final assistant message>" }
//! { "type": "run_aborted", "reason": "..." }
//! { "type": "error", "message": "..." }
//! ```
//!
//! Field → conclusion chain: *did it run* = a `run_result` record is present;
//! *success/fail* = `finishReason == "completed"`; *why* = any other
//! `finishReason`, a `run_aborted` record, or an `error` event.
//!
//! Deciding what a non-`completed` finish reason or a token/usage limit *means*
//! for a consumer is deliberately left to the caller — this crate reports the
//! CLI's own output faithfully and nothing more.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::Deserialize;
use serde::de::Error as DeError;
use serde::{Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::cli::Error;

/// One line from `cline --json`, preserved losslessly.
///
/// The underlying JSON object is retained verbatim so no field is lost across
/// Cline versions; typed accessors read through it.
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

    /// Classified event type, reading `type` (falling back to `kind`).
    #[must_use]
    pub fn event_type(&self) -> EventType {
        EventType::from_marker(self.type_marker())
    }

    fn type_marker(&self) -> Option<&str> {
        let object = self.raw.as_object()?;
        object
            .get("type")
            .or_else(|| object.get("kind"))
            .and_then(Value::as_str)
    }

    /// Does this event carry the given `type`/`kind`?
    fn type_is(&self, marker: &str) -> bool {
        self.type_marker() == Some(marker)
    }

    /// First string value among `keys` on the raw object.
    fn first_string(&self, keys: &[&str]) -> Option<String> {
        let object = self.raw.as_object()?;
        keys.iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
    }

    /// A human-readable summary of a failure event, reading `message`, then
    /// `reason`, then `error`. Empty when none is present.
    fn failure_summary(&self) -> String {
        self.first_string(&["message", "reason", "error"])
            .unwrap_or_default()
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
            Err(D::Error::custom("cline run event must be a JSON object"))
        }
    }
}

/// Known `cline --json` record types.
///
/// Cline emits a `run_result` terminal record plus `run_aborted` / `error`
/// failure events, interleaved with progress records this crate does not
/// classify individually. Unknown types fall through to [`Other`].
///
/// [`Other`]: EventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `run_result` — the terminal record carrying the finish reason.
    RunResult,
    /// `run_aborted` — the run was aborted before completing.
    RunAborted,
    /// `error` — an error event.
    Error,
    /// A record whose `type`/`kind` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("run_result") => Self::RunResult,
            Some("run_aborted") => Self::RunAborted,
            Some("error") => Self::Error,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// Finish reason reported in the terminal `run_result` record's `finishReason`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FinishReason {
    /// `finishReason: "completed"`.
    Completed,
    /// Any other finish-reason string the CLI emits (empty when the field was
    /// absent).
    Other(String),
}

impl FinishReason {
    fn parse(finish_reason: Option<&str>) -> Self {
        match finish_reason {
            Some("completed") => Self::Completed,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Other(String::new()),
        }
    }

    /// Is this the successful finish reason?
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// The finish reason as Cline's own label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Other(value) => value,
        }
    }
}

/// The terminal `run_result` record of a completed `cline --json` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunVerdict {
    /// Parsed `finishReason`.
    pub finish_reason: FinishReason,
    /// `sessionId` from the record, when present.
    pub session_id: Option<String>,
    /// Final assistant `message`, when present.
    pub message: Option<String>,
}

/// Raw terminal `run_result` record, deserialized.
#[derive(Debug, Default, Deserialize)]
struct RawRunVerdict {
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Parsed `cline --json` output: the event log plus derived accessors for the
/// terminal verdict.
///
/// Non-JSON and non-object lines in the stream are skipped, mirroring how
/// Cline interleaves the machine-readable records with incidental text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    events: Vec<Event>,
}

impl RunOutput {
    /// Parse a `cline --json` stdout stream leniently: every JSON-object line
    /// becomes an [`Event`], and any other line is ignored.
    #[must_use]
    pub fn parse(stdout: &str) -> Self {
        let events = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(Value::is_object)
            .map(Event::from_value)
            .collect();
        Self { events }
    }

    /// The parsed events, in stream order.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    fn last_matching<F>(&self, predicate: F) -> Option<&Event>
    where
        F: Fn(&Event) -> bool,
    {
        self.events.iter().rev().find(|event| predicate(event))
    }

    /// The terminal `run_result` record, when the stream produced one.
    ///
    /// The last `run_result` is authoritative: a completed run may be preceded
    /// by transient `error` events that do not override the terminal verdict.
    #[must_use]
    pub fn run_result(&self) -> Option<RunVerdict> {
        let event = self.last_matching(|event| event.type_is("run_result"))?;
        let record: RawRunVerdict =
            serde_json::from_value(event.as_value().clone()).unwrap_or_default();
        Some(RunVerdict {
            finish_reason: FinishReason::parse(record.finish_reason.as_deref()),
            session_id: record.session_id,
            message: record.message,
        })
    }

    /// Session id from the terminal `run_result` record, when present.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.run_result().and_then(|result| result.session_id)
    }

    /// Parsed finish reason from the terminal `run_result` record.
    #[must_use]
    pub fn finish_reason(&self) -> Option<FinishReason> {
        self.run_result().map(|result| result.finish_reason)
    }

    /// Final assistant message from the terminal `run_result` record.
    #[must_use]
    pub fn final_message(&self) -> Option<String> {
        self.run_result().and_then(|result| result.message)
    }

    /// Short human-readable failure summary from the last `run_aborted` record,
    /// or, failing that, the first `error` event.
    ///
    /// Returns `None` when the stream carries no failure event — inspect
    /// [`run_result`](Self::run_result) for the terminal verdict in that case.
    #[must_use]
    pub fn failure_message(&self) -> Option<String> {
        let aborted = self
            .last_matching(|event| event.type_is("run_aborted"))
            .map(Event::failure_summary);
        aborted.or_else(|| {
            self.events
                .iter()
                .find(|event| event.type_is("error"))
                .map(Event::failure_summary)
        })
    }
}

impl TryFrom<ProcessOutput> for RunOutput {
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

/// A live or collected stream of `cline --json` events.
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
