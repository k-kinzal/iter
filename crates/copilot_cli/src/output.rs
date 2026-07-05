//! Typed model of `copilot --output-format json`'s JSONL event stream.
//!
//! With `--output-format json`, Copilot streams newline-delimited JSON to
//! stdout — one JSON object per line. Two terminal record shapes carry the
//! verdict:
//!
//! ```jsonc
//! // success
//! { "type": "result", "sessionId": "<id>", "exitCode": 0, "usage": { "premiumRequests": 1 } }
//! // failure
//! { "type": "session.error", "errorType": "quota_exceeded", "errorCode": "...", "statusCode": 402 }
//! ```
//!
//! [`Event`] preserves each line losslessly; [`RunOutput`] collects the stream
//! and exposes the two terminal records ([`ResultRecord`], [`SessionError`]).
//!
//! What a `session.error` — or a non-zero exit — *means* for a consumer is
//! deliberately left to the caller: this crate reports the CLI's own output
//! faithfully and classifies nothing.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::cli::Error;

/// One line from `copilot --output-format json`, preserved losslessly.
///
/// The underlying JSON object is retained verbatim so no field is lost across
/// Copilot versions; typed accessors read through it.
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

    /// Classified event type, read from the top-level `type` (or `kind`).
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

    /// Does this event carry the given `type`/`kind` marker?
    fn type_is(&self, marker: &str) -> bool {
        self.type_marker() == Some(marker)
    }

    fn deserialize_as<T: for<'de> Deserialize<'de>>(&self) -> Option<T> {
        serde_json::from_value(self.raw.clone()).ok()
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
            Err(D::Error::custom("copilot event must be a JSON object"))
        }
    }
}

/// Known `copilot --output-format json` event types.
///
/// Only the two terminal records are classified by name; every other event
/// (progress, tool activity, …) falls through to [`Other`], since Copilot's
/// intermediate event vocabulary is not part of this crate's verified surface.
///
/// [`Other`]: EventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `result`: the terminal success record.
    Result,
    /// `session.error`: the terminal failure record.
    SessionError,
    /// An event whose `type`/`kind` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("result") => Self::Result,
            Some("session.error") => Self::SessionError,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// Usage figures from a `result` record's `usage` object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Usage {
    /// `usage.premiumRequests`, when reported.
    #[serde(default)]
    pub premium_requests: Option<u64>,
}

/// The terminal `result` record of a successful run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ResultRecord {
    /// `sessionId` from the terminal record.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `exitCode` reported inside the terminal record, when present.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Parsed `usage` figures.
    #[serde(default)]
    pub usage: Option<Usage>,
}

impl ResultRecord {
    /// Premium-request count from the `usage` object, when reported.
    #[must_use]
    pub fn premium_requests(&self) -> Option<u64> {
        self.usage.as_ref().and_then(|usage| usage.premium_requests)
    }
}

/// The terminal `session.error` record of a failed run.
///
/// Its **presence** in the stream is the failure signal — authoritative even
/// when a `result` record also appears. The CLI emits camelCase keys
/// (`errorType`, `errorCode`, `statusCode`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SessionError {
    /// `errorType` from the record.
    #[serde(default)]
    pub error_type: Option<String>,
    /// `errorCode` from the record, when present.
    #[serde(default)]
    pub error_code: Option<String>,
    /// `statusCode` (HTTP-ish) from the record, when present.
    #[serde(default)]
    pub status_code: Option<u16>,
}

/// Parsed `copilot --output-format json` output: the event log plus accessors
/// for the two terminal records.
///
/// Non-JSON and non-object lines in the stream are skipped, mirroring how
/// Copilot may interleave incidental text with the machine-readable events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    events: Vec<Event>,
}

impl RunOutput {
    /// Parse a JSONL stdout stream leniently: every JSON-object line becomes an
    /// [`Event`], and any other line is ignored.
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

    fn last_of_type(&self, marker: &str) -> Option<&Event> {
        self.events.iter().rev().find(|event| event.type_is(marker))
    }

    /// The terminal `result` record, from the last `result` event.
    #[must_use]
    pub fn result(&self) -> Option<ResultRecord> {
        self.last_of_type("result")
            .map(|event| event.deserialize_as().unwrap_or_default())
    }

    /// The terminal `session.error` record, from the last `session.error`
    /// event. Its presence is the failure signal.
    #[must_use]
    pub fn session_error(&self) -> Option<SessionError> {
        self.last_of_type("session.error")
            .map(|event| event.deserialize_as().unwrap_or_default())
    }

    /// Session id from the terminal `result` record, when present.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.result().and_then(|record| record.session_id)
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

/// A live or collected stream of `copilot --output-format json` events.
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
