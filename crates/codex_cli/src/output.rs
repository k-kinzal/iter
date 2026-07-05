//! Typed model of `codex exec --json`'s JSONL event stream.
//!
//! `codex exec --json` streams newline-delimited JSON events to stdout. Codex
//! wraps most events as `{"type": "...", ...}`, though some builds nest the
//! payload under a `"msg"` object and some use `"kind"` instead of `"type"` —
//! both shapes are tolerated. [`Event`] preserves each line losslessly and
//! exposes typed accessors; [`ExecOutput`] collects the stream and derives the
//! terminal verdict (session id, turn status, usage, error message).
//!
//! Deciding what a non-zero exit or a token/usage limit *means* for a consumer
//! is deliberately left to the caller — this crate reports the CLI's own
//! output faithfully and nothing more.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::cli::Error;

/// One line from `codex exec --json`, preserved losslessly.
///
/// The underlying JSON object is retained verbatim so no field is lost across
/// Codex versions; typed accessors read through both the flat and the
/// `msg`-wrapped event shapes.
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

    /// Classified event type, reading `type`/`kind` at the top level or under
    /// a nested `msg` object.
    #[must_use]
    pub fn event_type(&self) -> EventType {
        EventType::from_marker(self.type_marker())
    }

    /// The event payload, unwrapping a nested `msg` object when present so
    /// field lookups work for both the flat and the wrapped shapes.
    #[must_use]
    pub fn payload(&self) -> &Value {
        self.raw
            .get("msg")
            .filter(|value| value.is_object())
            .unwrap_or(&self.raw)
    }

    fn type_marker(&self) -> Option<&str> {
        fn marker(value: &Value) -> Option<&str> {
            let object = value.as_object()?;
            object
                .get("type")
                .or_else(|| object.get("kind"))
                .and_then(Value::as_str)
        }
        marker(&self.raw).or_else(|| self.raw.get("msg").and_then(marker))
    }

    /// Does this event (flat or `msg`-wrapped) carry the given `type`/`kind`?
    #[must_use]
    fn type_is(&self, marker: &str) -> bool {
        let direct = self
            .raw
            .as_object()
            .and_then(|object| object.get("type").or_else(|| object.get("kind")))
            .and_then(Value::as_str)
            == Some(marker);
        let nested = self
            .raw
            .get("msg")
            .and_then(Value::as_object)
            .and_then(|object| object.get("type").or_else(|| object.get("kind")))
            .and_then(Value::as_str)
            == Some(marker);
        direct || nested
    }

    /// First string value among `keys` on the event payload.
    fn first_string(&self, keys: &[&str]) -> Option<String> {
        let payload = self.payload();
        keys.iter()
            .find_map(|key| payload.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
    }

    fn item_type(&self) -> Option<&str> {
        self.payload()
            .get("item")
            .and_then(Value::as_object)
            .and_then(|item| item.get("type").or_else(|| item.get("item_type")))
            .and_then(Value::as_str)
    }

    /// Does this event look like a terminal turn-status record?
    fn is_turn_status(&self) -> bool {
        if self.type_is("task_complete")
            || self.type_is("turn_complete")
            || self.type_is("turn.completed")
            || self.type_is("turn.failed")
            || self.type_is("turn_status")
            || self.type_is("error")
        {
            return true;
        }
        let status = self
            .payload()
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        matches!(
            status.as_deref(),
            Some("completed" | "failed" | "interrupted")
        )
    }

    fn turn_status_str(&self) -> Option<String> {
        self.first_string(&["status", "turn_status"])
    }

    /// Classify a terminal turn record. Codex 0.139.0's flat schema makes the
    /// event type authoritative when no `status` field is present.
    fn turn_status(&self) -> TurnStatus {
        if self.type_is("turn.completed") {
            return TurnStatus::Completed;
        }
        if self.type_is("turn.failed") || self.type_is("error") {
            return TurnStatus::Failed;
        }
        if let Some(status) = self.turn_status_str() {
            return TurnStatus::parse(&status);
        }
        if self.type_is("task_complete") || self.type_is("turn_complete") {
            return TurnStatus::Completed;
        }
        TurnStatus::Other("unknown".to_owned())
    }

    fn turn_status_label(&self, status: &TurnStatus) -> String {
        self.turn_status_str().unwrap_or_else(|| match status {
            TurnStatus::Completed => "completed".to_owned(),
            TurnStatus::Failed => "failed".to_owned(),
            TurnStatus::Interrupted => "interrupted".to_owned(),
            TurnStatus::Other(value) => value.clone(),
        })
    }

    fn is_agent_message(&self) -> bool {
        self.type_is("agent_message")
            || self.type_is("agent.message")
            || (self.type_is("item.completed") && self.item_type() == Some("agent_message"))
    }

    fn agent_message_text(&self) -> Option<String> {
        self.first_string(&["message", "text", "last_agent_message"])
            .or_else(|| {
                self.payload()
                    .get("item")
                    .and_then(|item| item.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    }

    fn total_tokens(&self) -> Option<u64> {
        let payload = self.payload();
        payload
            .get("total_tokens")
            .or_else(|| payload.get("total_token_count"))
            .or_else(|| payload.pointer("/usage/total_tokens"))
            .and_then(Value::as_u64)
            .or_else(|| {
                let input = payload
                    .pointer("/usage/input_tokens")
                    .and_then(Value::as_u64)?;
                let output = payload
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)?;
                input.checked_add(output)
            })
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
            Err(D::Error::custom("codex exec event must be a JSON object"))
        }
    }
}

/// Known `codex exec --json` event types.
///
/// Codex 0.139.0 emits the `thread.started` / `turn.started` / `item.*` /
/// `turn.completed` / `turn.failed` / `error` family; the legacy
/// `session_configured` / `agent_message` / `task_complete` / `token_count`
/// shapes are also recognized. Unknown types fall through to [`Other`].
///
/// [`Other`]: EventType::Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `thread.started`.
    ThreadStarted,
    /// `turn.started`.
    TurnStarted,
    /// `item.started`.
    ItemStarted,
    /// `item.updated`.
    ItemUpdated,
    /// `item.completed`.
    ItemCompleted,
    /// `turn.completed`.
    TurnCompleted,
    /// `turn.failed`.
    TurnFailed,
    /// `error`.
    Error,
    /// Legacy `session_configured`.
    SessionConfigured,
    /// Legacy `agent_message`.
    AgentMessage,
    /// Legacy `task_complete`.
    TaskComplete,
    /// Legacy `turn_complete`.
    TurnComplete,
    /// Legacy `token_count`.
    TokenCount,
    /// Legacy `token_usage`.
    TokenUsage,
    /// An event whose `type`/`kind` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("thread.started") => Self::ThreadStarted,
            Some("turn.started") => Self::TurnStarted,
            Some("item.started") => Self::ItemStarted,
            Some("item.updated") => Self::ItemUpdated,
            Some("item.completed") => Self::ItemCompleted,
            Some("turn.completed") => Self::TurnCompleted,
            Some("turn.failed") => Self::TurnFailed,
            Some("error") => Self::Error,
            Some("session_configured") => Self::SessionConfigured,
            Some("agent_message") => Self::AgentMessage,
            Some("task_complete") => Self::TaskComplete,
            Some("turn_complete") => Self::TurnComplete,
            Some("token_count") => Self::TokenCount,
            Some("token_usage") => Self::TokenUsage,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// Terminal turn status reported by Codex's turn-status record.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TurnStatus {
    /// Turn finished successfully.
    Completed,
    /// Turn ended in failure.
    Failed,
    /// Turn was interrupted.
    Interrupted,
    /// Any other status string the CLI emits.
    Other(String),
}

impl TurnStatus {
    fn parse(status: &str) -> Self {
        match status.to_ascii_lowercase().as_str() {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => Self::Other(status.to_owned()),
        }
    }

    /// Is this the successful terminal status?
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// The status as Codex's lowercase label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Other(value) => value,
        }
    }
}

/// The terminal turn-status record of a completed `codex exec` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnVerdict {
    /// The parsed status.
    pub status: TurnStatus,
    /// Codex's own label for the status, preserved verbatim.
    pub label: String,
    /// Error message carried by a failing record, when present.
    pub error_message: Option<String>,
    /// `will_retry` flag from the accompanying error item.
    pub will_retry: bool,
}

/// Parsed `codex exec --json` output: the event log plus derived accessors for
/// the terminal verdict.
///
/// Non-JSON and non-object lines in the stream are skipped, mirroring how
/// Codex interleaves the machine-readable events with incidental text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    events: Vec<Event>,
}

impl ExecOutput {
    /// Parse a `codex exec --json` stdout stream leniently: every JSON-object
    /// line becomes an [`Event`], and any other line is ignored.
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

    /// Session / conversation / thread id, from the latest event exposing one.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        let keys = &["session_id", "conversation_id", "thread_id"];
        self.last_matching(|event| event.first_string(keys).is_some())
            .and_then(|event| event.first_string(keys))
    }

    /// Final assistant message text, from the latest agent-message event.
    #[must_use]
    pub fn final_message(&self) -> Option<String> {
        self.last_matching(Event::is_agent_message)
            .and_then(Event::agent_message_text)
    }

    /// Total tokens used, from the latest usage-bearing event.
    #[must_use]
    pub fn total_tokens(&self) -> Option<u64> {
        self.last_matching(|event| {
            event.type_is("token_count")
                || event.type_is("token_usage")
                || event.type_is("turn.completed")
        })
        .and_then(Event::total_tokens)
    }

    /// The terminal turn-status record, when the stream produced one.
    #[must_use]
    pub fn turn_outcome(&self) -> Option<TurnVerdict> {
        let event = self.last_matching(Event::is_turn_status)?;
        let status = event.turn_status();
        let label = event.turn_status_label(&status);
        let error_message = event
            .payload()
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| event.first_string(&["message"]));
        Some(TurnVerdict {
            status,
            label,
            error_message,
            will_retry: self.will_retry(),
        })
    }

    /// Convenience: the terminal turn status alone.
    #[must_use]
    pub fn terminal_status(&self) -> Option<TurnStatus> {
        self.turn_outcome().map(|outcome| outcome.status)
    }

    /// `will_retry` flag from the latest error item, when present.
    #[must_use]
    pub fn will_retry(&self) -> bool {
        self.last_matching(|event| event.type_is("error"))
            .is_some_and(|event| {
                event
                    .payload()
                    .get("will_retry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
    }
}

impl TryFrom<ProcessOutput> for ExecOutput {
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

/// A live or collected stream of `codex exec --json` events.
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
