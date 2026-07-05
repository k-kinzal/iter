//! Typed model of `grok -p … --output-format json` / `streaming-json` output.
//!
//! Verified against `grok 0.2.82` headless. With `--output-format json` the
//! whole stream is a single terminal JSON object; with
//! `--output-format streaming-json` it is a newline-delimited event stream that
//! terminates with a `{"type":"end", …}` event carrying the same metadata.
//! [`SingleOutput`] models both: it locates the terminal object (whole-stream
//! object first, then a streaming `end`/`result` event) and exposes typed
//! accessors over it, while preserving the raw stdout so an in-band `error`
//! event anywhere in a streaming run is still surfaced.
//!
//! Deciding what a non-zero exit or a token-limit phrase *means* for a consumer
//! is deliberately left to the caller — this crate reports the CLI's own output
//! faithfully and nothing more.
//!
//! ## Output contract (Grok Build, `--output-format json`)
//!
//! ```jsonc
//! {
//!   "text":       "<final assistant message>",
//!   "stopReason": "EndTurn",         // stop / finish reason
//!   "sessionId":  "<uuid>",          // camelCase
//!   "requestId":  "<uuid>",          // server-side request id for this turn
//!   "thought":    "<reasoning text>" // present only when reasoning is shown
//! }
//! ```
//!
//! On failure Grok emits `{"type":"error","message":"…"}` instead, so the
//! `type` discriminator is checked before the payload is read as a success.
//!
//! **No usage/cost in this revision.** `grok 0.2.82` reports *no* token-count
//! or cost fields in the headless JSON object (confirmed against the shipped
//! binary and `~/.grok/docs/user-guide/14-headless-mode.md`). [`Usage`]
//! therefore parses such fields *defensively* — tolerating the plausible
//! `camelCase`/`snake_case` names a future revision might use — and yields an
//! empty usage when, as today, none are present.

use std::fmt;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::cli::Error;

/// Parse `stdout` as a single JSON value (the whole `--output-format json`
/// stream is one document). `None` when empty or not valid JSON.
///
/// Reimplemented locally so this OSS crate depends on nothing internal.
fn single_object(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Return the **last** JSON-object line whose `type`/`kind` equals `marker`.
///
/// Reimplemented locally so this OSS crate depends on nothing internal.
fn last_event_of_type(stdout: &str, marker: &str) -> Option<Value> {
    last_event_matching(stdout, |obj| {
        obj.get("type")
            .or_else(|| obj.get("kind"))
            .and_then(Value::as_str)
            == Some(marker)
    })
}

/// Return the **last** JSON-object line for which `pred` holds. Non-object
/// lines are skipped.
fn last_event_matching<F>(stdout: &str, pred: F) -> Option<Value>
where
    F: Fn(&Map<String, Value>) -> bool,
{
    let mut found = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line)
            && pred(&obj)
        {
            found = Some(Value::Object(obj));
        }
    }
    found
}

/// First present string among `keys` on `obj`.
fn first_str(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

/// Detect an in-band error / refusal in a terminal object, returning a
/// human-readable summary when the object indicates a failure.
fn reported_error_of(obj: &Value) -> Option<String> {
    // The verified failure shape: `{"type":"error","message":"…"}`. The `type`
    // discriminator must be honored before the object is read as a success,
    // otherwise the error `message` would be mistaken for a result.
    if obj.get("type").and_then(Value::as_str) == Some("error") {
        return Some(first_str(obj, &["message", "error"]).unwrap_or_else(|| "error".to_owned()));
    }
    // An explicit `error` object/string.
    if let Some(error) = obj.get("error") {
        match error {
            Value::String(text) if !text.is_empty() => return Some(text.clone()),
            Value::Object(_) => {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| "error".to_owned(), str::to_owned);
                return Some(message);
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            }
        }
    }
    // A boolean error flag (`isError` / `is_error`) paired with the message.
    let flagged = obj
        .get("isError")
        .or_else(|| obj.get("is_error"))
        .and_then(Value::as_bool)
        == Some(true);
    if flagged {
        return Some(final_message_of(obj).unwrap_or_else(|| "error".to_owned()));
    }
    None
}

/// Read the final response text. `text` is what `grok 0.2.82` emits; the rest
/// are defensive fallbacks since the name is not pinned beyond `sessionId`.
fn final_message_of(obj: &Value) -> Option<String> {
    first_str(obj, &["text", "response", "result", "message", "output"])
}

/// Stop / finish reason reported in the terminal result object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// A normal stop (`stopReason: "EndTurn"`; legacy `finishReason: "stop"` /
    /// `"end_turn"`).
    Stop,
    /// Any other finish-reason string the CLI emits.
    Other(String),
    /// No finish reason was reported.
    Unknown,
}

impl StopReason {
    fn parse(reason: Option<&str>) -> Self {
        match reason {
            // `EndTurn` is the value `grok 0.2.82` reports on a normal stop; the
            // snake/lower forms are kept for cross-revision tolerance.
            Some("EndTurn" | "stop" | "end_turn") => Self::Stop,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Unknown,
        }
    }

    /// Is this a normal end-of-turn stop?
    #[must_use]
    pub const fn is_stop(&self) -> bool {
        matches!(self, Self::Stop)
    }
}

/// Token-usage / cost reported in the terminal object.
///
/// `grok 0.2.82` headless JSON reports **none** of these, so every field is
/// normally `None`; this type exists so the values are captured rather than
/// silently dropped if a future Grok revision (or a model path that does
/// surface accounting) starts reporting them. Each field tolerates the
/// plausible `camelCase`/`snake_case` spellings; both a `usage`-like nesting
/// and the top-level object are searched (see [`Usage::parse`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Prompt / input token count, when reported.
    pub input_tokens: Option<u64>,
    /// Completion / output token count, when reported.
    pub output_tokens: Option<u64>,
    /// Total token count, when reported.
    pub total_tokens: Option<u64>,
    /// Reported run cost in USD, when reported.
    pub total_cost_usd: Option<f64>,
}

impl Usage {
    /// Parse token-usage / cost defensively. Both a `usage`-like sub-object and
    /// the top-level object are searched, so a flat or a nested report both
    /// resolve. Name specificity wins over scope: each field tries its names
    /// most-canonical-first across *every* scope before falling through to the
    /// next alias, so a generic `cost` nested under `usage` never shadows a
    /// canonical `total_cost_usd` at the top level. For one *same* name present
    /// in both scopes, the nested `usage` object is preferred. Yields an empty
    /// [`Usage`] when nothing is reported — the `grok 0.2.82` case.
    fn parse(root: &Value) -> Self {
        let nested = ["usage", "tokenUsage", "token_usage"]
            .iter()
            .find_map(|key| root.get(*key));
        let scopes: Vec<&Value> = nested.into_iter().chain(std::iter::once(root)).collect();
        let u64_of = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                scopes
                    .iter()
                    .find_map(|scope| scope.get(*key).and_then(Value::as_u64))
            })
        };
        let f64_of = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                scopes
                    .iter()
                    .find_map(|scope| scope.get(*key).and_then(Value::as_f64))
            })
        };
        Self {
            input_tokens: u64_of(&["input_tokens", "inputTokens", "prompt_tokens", "promptTokens"]),
            output_tokens: u64_of(&[
                "output_tokens",
                "outputTokens",
                "completion_tokens",
                "completionTokens",
            ]),
            total_tokens: u64_of(&["total_tokens", "totalTokens"]),
            total_cost_usd: f64_of(&["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd", "cost"]),
        }
    }

    /// Whether any usage/cost field was reported.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Parsed `grok -p … --output-format json` output: the terminal result object
/// (when the run produced one) plus derived accessors, and the raw stdout so an
/// in-band streaming `error` event is still reachable.
///
/// [`SingleOutput::parse`] always succeeds — [`terminal`](Self::terminal) is
/// `None` when the run produced no result object (e.g. it died before writing
/// one). [`TryFrom<ProcessOutput>`] adds the exit-status verdict: no terminal
/// object plus a non-zero exit becomes [`Error::Cli`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleOutput {
    terminal: Option<Value>,
    stdout: String,
}

impl SingleOutput {
    /// Parse a `grok` headless stdout stream. The terminal object is the
    /// whole-stream JSON object, else a streaming `end`/`result` event; the raw
    /// stdout is retained so a streaming `error` event is still surfaced.
    #[must_use]
    pub fn parse(stdout: &str) -> Self {
        // Whole-stream JSON object first; then the `streaming-json` terminal
        // event (`type:"end"` in `grok 0.2.82`), then a legacy `result` event.
        let terminal = single_object(stdout)
            .or_else(|| last_event_of_type(stdout, "end"))
            .or_else(|| last_event_of_type(stdout, "result"));
        Self {
            terminal,
            stdout: stdout.to_owned(),
        }
    }

    /// The terminal result object, when the run produced one.
    #[must_use]
    pub fn terminal(&self) -> Option<&Value> {
        self.terminal.as_ref()
    }

    /// `sessionId` from the terminal object (camelCase per the contract).
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.terminal
            .as_ref()?
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// `requestId` — the server-side id for this turn.
    #[must_use]
    pub fn request_id(&self) -> Option<String> {
        first_str(self.terminal.as_ref()?, &["requestId", "request_id"])
    }

    /// Final assistant message (`text`), when reported.
    #[must_use]
    pub fn final_message(&self) -> Option<String> {
        final_message_of(self.terminal.as_ref()?)
    }

    /// Reasoning text (`thought`), present only when reasoning is shown.
    #[must_use]
    pub fn thought(&self) -> Option<String> {
        first_str(self.terminal.as_ref()?, &["thought", "reasoning"])
    }

    /// Parsed finish reason. [`StopReason::Unknown`] when none was reported.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        let reason = self.terminal.as_ref().and_then(|obj| {
            // The verified `stopReason` is probed ahead of the legacy
            // `finishReason` so an object carrying both resolves canonically.
            for key in ["stopReason", "stop_reason", "finishReason", "finish_reason"] {
                if let Some(reason) = obj.get(key).and_then(Value::as_str) {
                    return Some(reason.to_owned());
                }
            }
            None
        });
        StopReason::parse(reason.as_deref())
    }

    /// Parsed token-usage / cost (empty on `grok 0.2.82`).
    #[must_use]
    pub fn usage(&self) -> Usage {
        self.terminal.as_ref().map(Usage::parse).unwrap_or_default()
    }

    /// An in-band error / refusal, when the run reported one: either the
    /// terminal object itself reports an error, or — in `streaming-json` — a
    /// `type:"error"` event was emitted alongside the success-looking terminal
    /// `end` event and must not be swallowed. Any error event in the stream is
    /// treated as a failure (fail-safe); the *last* one is surfaced when several
    /// are present (e.g. transient retries before a fatal error).
    #[must_use]
    pub fn reported_error(&self) -> Option<String> {
        self.terminal
            .as_ref()
            .and_then(reported_error_of)
            .or_else(|| last_event_of_type(&self.stdout, "error").and_then(|e| reported_error_of(&e)))
    }
}

impl TryFrom<ProcessOutput> for SingleOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = Self::parse(&stdout);
        if parsed.terminal.is_none() && !output.status.success() {
            return Err(Error::Cli {
                exit_code: output.status.code(),
                stdout: stdout.into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(parsed)
    }
}

/// One line from `grok -p … --output-format streaming-json`, preserved
/// losslessly. The underlying JSON object is retained verbatim so no field is
/// lost across Grok versions; typed accessors read through it.
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

    /// Classified event type, reading `type` (or `kind`).
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

    /// Incremental text (`data`/`text`) carried by a `text` event.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        first_str(&self.raw, &["data", "text"])
    }

    /// An error message carried by an `error` event, if any.
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        reported_error_of(&self.raw)
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
            Err(D::Error::custom("grok streaming event must be a JSON object"))
        }
    }
}

/// Known `grok … --output-format streaming-json` event types.
///
/// `grok 0.2.82` emits `text` deltas terminated by an `end` event, with an
/// `error` event on failure; the legacy `result` shape is also recognized.
/// Unknown types fall through to [`Other`](EventType::Other).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// `text` — an incremental assistant-text delta.
    Text,
    /// `end` — the terminal event carrying stop/session metadata.
    End,
    /// `error` — an in-band error / refusal.
    Error,
    /// Legacy `result` terminal event.
    Result,
    /// An event whose `type`/`kind` was absent or unrecognized.
    Other(Option<String>),
}

impl EventType {
    fn from_marker(marker: Option<&str>) -> Self {
        match marker {
            Some("text") => Self::Text,
            Some("end") => Self::End,
            Some("error") => Self::Error,
            Some("result") => Self::Result,
            Some(other) => Self::Other(Some(other.to_owned())),
            None => Self::Other(None),
        }
    }
}

/// A live or collected stream of `grok … --output-format streaming-json`
/// events.
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
