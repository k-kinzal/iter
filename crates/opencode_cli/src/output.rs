//! Typed model of `opencode run --format json`'s event stream.
//!
//! opencode's `run --format json` emits a machine-readable event stream. The
//! shape varies by build: a single JSON object for a whole run, or a
//! newline-delimited sequence of `{"type": "...", ...}` records. [`RunOutput`]
//! accepts both, preserves each event losslessly, and exposes typed accessors
//! for the pieces a consumer cares about — the session id, the final assistant
//! message, and whether the run surfaced an error event.
//!
//! # The exit code lies
//!
//! opencode is one of the exit-0-but-failed CLIs: the process can exit `0`
//! while the run failed. The authoritative failure signal is the **presence of
//! an error event** (`session.error` or `result.error`) in the stream, not the
//! exit code. This module reports that presence faithfully; deciding what it
//! *means* — a generic failure, a token-limit class, a signal — is left to the
//! caller.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::cli::Error;

/// One event from `opencode run --format json`, preserved losslessly.
///
/// The underlying JSON object is retained verbatim so no field is lost across
/// opencode versions; typed accessors read the known fields on top.
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

    /// Classified event type, reading the `type`/`kind` marker.
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
    #[must_use]
    fn type_is(&self, marker: &str) -> bool {
        self.type_marker() == Some(marker)
    }

    /// Is this a `session.error` / `result.error` record?
    #[must_use]
    fn is_error_event(&self) -> bool {
        self.type_is("session.error") || self.type_is("result.error")
    }

    /// Is this a `session` record?
    #[must_use]
    fn is_session(&self) -> bool {
        self.type_is("session")
    }

    /// First string value among `keys` on the raw object.
    fn first_string(&self, keys: &[&str]) -> Option<String> {
        let object = self.raw.as_object()?;
        keys.iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
    }

    /// The session id carried on this record, from `id` / `sessionId` /
    /// `session_id`.
    fn session_id(&self) -> Option<String> {
        self.first_string(&["id", "sessionId", "session_id"])
    }

    /// The human-readable message on an error event: opencode nests it under
    /// `error.message`, but a flat top-level `message` is tolerated too.
    fn error_message(&self) -> String {
        self.raw
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.first_string(&["message"]))
            .unwrap_or_default()
    }

    /// Assistant text on a `result` / `session` record, from `text` or
    /// `message`.
    fn message_text(&self) -> Option<String> {
        self.first_string(&["text", "message"])
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
            Err(D::Error::custom("opencode run event must be a JSON object"))
        }
    }
}

/// Known `opencode run --format json` event types.
///
/// opencode surfaces a `session` record when a run reaches its idle terminal
/// state, and a `session.error` / `result.error` record when a run fails; a
/// `result` record may carry the final assistant text. Unknown types fall
/// through to [`Other`].
///
/// [`Other`]: EventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `session` — a session state record (reaches `idle` on a clean run).
    Session,
    /// `session.error` — an in-band failure on the exit-0-but-failed path.
    SessionError,
    /// `result.error` — an in-band failure on the synchronous exit-1 path.
    ResultError,
    /// `result` — a terminal result record carrying the assistant text.
    Result,
    /// An event whose `type`/`kind` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("session") => Self::Session,
            Some("session.error") => Self::SessionError,
            Some("result.error") => Self::ResultError,
            Some("result") => Self::Result,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// An error event recovered from an `opencode run --format json` stream.
///
/// Reports the message opencode carried on a `session.error` / `result.error`
/// record. The message may be empty when the CLI emitted a bare error event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    message: String,
}

impl RunError {
    /// The error message recovered from the event (possibly empty).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Consume the report, returning the owned message.
    #[must_use]
    pub fn into_message(self) -> String {
        self.message
    }
}

/// Parsed `opencode run --format json` output: the event log plus derived
/// accessors for the terminal verdict.
///
/// Both stream shapes are accepted — a whole-stream single JSON object and a
/// newline-delimited sequence of object events. Non-object lines are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    events: Vec<Event>,
}

impl RunOutput {
    /// Parse an `opencode run --format json` stdout stream leniently.
    ///
    /// A whole-stream single JSON object (the `--output-format json` contract)
    /// becomes a one-event log; otherwise each JSON-object line becomes an
    /// [`Event`] and any other line is ignored.
    #[must_use]
    pub fn parse(stdout: &str) -> Self {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Self { events: Vec::new() };
        }
        // The single-document form: the whole stream is one JSON object.
        if let Ok(value) = serde_json::from_str::<Value>(trimmed)
            && value.is_object()
        {
            return Self {
                events: vec![Event::from_value(value)],
            };
        }
        // Otherwise a JSON-lines stream: one object per line.
        let events = trimmed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
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

    fn first_matching<F>(&self, predicate: F) -> Option<&Event>
    where
        F: Fn(&Event) -> bool,
    {
        self.events.iter().find(|event| predicate(event))
    }

    /// The session id, preferring a `session` record and falling back to any
    /// record exposing an `id` / `sessionId` / `session_id`.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.last_matching(|event| event.is_session() && event.session_id().is_some())
            .or_else(|| self.last_matching(|event| event.session_id().is_some()))
            .and_then(Event::session_id)
    }

    /// The final assistant message text, from the latest `result` / `session`
    /// record carrying `text` or `message`.
    #[must_use]
    pub fn final_message(&self) -> Option<String> {
        self.last_matching(|event| event.type_is("result") && event.message_text().is_some())
            .or_else(|| {
                self.last_matching(|event| event.is_session() && event.message_text().is_some())
            })
            .and_then(Event::message_text)
    }

    /// Whether the stream surfaced an error event.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.events.iter().any(Event::is_error_event)
    }

    /// The first error event in the stream, when one is present.
    ///
    /// The presence of this — regardless of the exit code — is opencode's
    /// authoritative failure signal.
    #[must_use]
    pub fn error(&self) -> Option<RunError> {
        self.first_matching(Event::is_error_event)
            .map(|event| RunError {
                message: event.error_message(),
            })
    }
}

impl TryFrom<ProcessOutput> for RunOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = Self::parse(&stdout);
        // opencode's exit code lies, so a non-empty stream is authoritative
        // even on a non-zero exit. Only an empty stream on a failed exit is a
        // launch/pre-flight crash with nothing machine-readable to report.
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

/// A live or collected stream of `opencode run --format json` events.
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
