use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::process::Output as ProcessOutput;
use std::task::{Context, Poll};

use futures::{Stream, stream};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::cli::Error;

fn cli_error(output: &ProcessOutput) -> Error {
    Error::Cli {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Text emitted by Claude Code stdout.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextOutput(String);

impl TextOutput {
    /// Borrow the captured stdout text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the captured stdout text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for TextOutput {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<TextOutput> for String {
    fn from(output: TextOutput) -> Self {
        output.into_string()
    }
}

impl TryFrom<ProcessOutput> for TextOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        if output.status.success() {
            Ok(Self(String::from_utf8_lossy(&output.stdout).into_owned()))
        } else {
            Err(cli_error(&output))
        }
    }
}

/// Terminal result emitted by `claude --print --output-format json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonOutput {
    /// Always `result` for the terminal JSON object.
    #[serde(rename = "type")]
    pub output_type: JsonOutputType,
    /// Result classification reported by Claude Code.
    #[serde(default, deserialize_with = "deserialize_result_subtype")]
    pub subtype: ResultSubtype,
    /// Whether the result represents an error.
    #[serde(default, deserialize_with = "deserialize_bool_lossy")]
    pub is_error: bool,
    /// Final assistant response text, when present.
    #[serde(
        default,
        deserialize_with = "deserialize_option_string_lossy",
        skip_serializing_if = "Option::is_none"
    )]
    pub result: Option<String>,
    /// Claude Code session id.
    #[serde(
        default,
        deserialize_with = "deserialize_option_string_lossy",
        skip_serializing_if = "Option::is_none"
    )]
    pub session_id: Option<String>,
    /// Number of turns used by the run.
    #[serde(
        default,
        deserialize_with = "deserialize_option_u64_lossy",
        skip_serializing_if = "Option::is_none"
    )]
    pub num_turns: Option<u64>,
    /// Reported API cost.
    #[serde(
        default,
        deserialize_with = "deserialize_option_f64_lossy",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_cost_usd: Option<f64>,
    /// Usage payload. Claude Code may extend this shape between versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    /// Forward-compatible fields preserved for callers.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TryFrom<ProcessOutput> for JsonOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        match serde_json::from_slice(&output.stdout) {
            Ok(result) => Ok(result),
            Err(_) if !output.status.success() => Err(cli_error(&output)),
            Err(err) => Err(Error::Json(err)),
        }
    }
}

/// Discriminator for `claude --print --output-format json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsonOutputType {
    /// `result`.
    Result,
}

impl Serialize for JsonOutputType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("result")
    }
}

impl<'de> Deserialize<'de> for JsonOutputType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "result" {
            Ok(Self::Result)
        } else {
            Err(D::Error::custom(format!(
                "expected result event type, got {value}"
            )))
        }
    }
}

/// Known `subtype` values for JSON result output.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResultSubtype {
    /// Successful run.
    Success,
    /// The run exceeded max turns.
    ErrorMaxTurns,
    /// Claude Code reported execution failure.
    ErrorDuringExecution,
    /// Any new subtype introduced by Claude Code.
    Other(String),
}

impl ResultSubtype {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Success => "success",
            Self::ErrorMaxTurns => "error_max_turns",
            Self::ErrorDuringExecution => "error_during_execution",
            Self::Other(value) => value,
        }
    }
}

impl Default for ResultSubtype {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl Serialize for ResultSubtype {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ResultSubtype {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "success" => Self::Success,
            "error_max_turns" => Self::ErrorMaxTurns,
            "error_during_execution" => Self::ErrorDuringExecution,
            _ => Self::Other(value),
        })
    }
}

fn deserialize_result_subtype<'de, D>(deserializer: D) -> Result<ResultSubtype, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(value)) => match value.as_str() {
            "success" => ResultSubtype::Success,
            "error_max_turns" => ResultSubtype::ErrorMaxTurns,
            "error_during_execution" => ResultSubtype::ErrorDuringExecution,
            _ => ResultSubtype::Other(value),
        },
        _ => ResultSubtype::default(),
    })
}

fn deserialize_bool_lossy<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

fn deserialize_option_string_lossy<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<Value>::deserialize(deserializer)?.and_then(|value| match value {
            Value::String(value) => Some(value),
            _ => None,
        }),
    )
}

fn deserialize_option_u64_lossy<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?.and_then(|value| value.as_u64()))
}

fn deserialize_option_f64_lossy<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Value>::deserialize(deserializer)?.and_then(|value| value.as_f64()))
}

/// One line from `claude --print --output-format stream-json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Event discriminator.
    #[serde(rename = "type")]
    pub event_type: StreamEventType,
    /// Forward-compatible event payload.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// Known stream-json event types.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamEventType {
    /// Initial system metadata.
    System,
    /// Assistant message payload.
    Assistant,
    /// User message payload.
    User,
    /// Terminal result payload.
    Result,
    /// Hook lifecycle payload.
    Hook,
    /// Prompt suggestion payload.
    PromptSuggestion,
    /// Rate limit status payload.
    RateLimitEvent,
    /// Any new event type introduced by Claude Code.
    Other(String),
}

impl StreamEventType {
    #[must_use]
    fn as_str(&self) -> &str {
        match self {
            Self::System => "system",
            Self::Assistant => "assistant",
            Self::User => "user",
            Self::Result => "result",
            Self::Hook => "hook",
            Self::PromptSuggestion => "prompt_suggestion",
            Self::RateLimitEvent => "rate_limit_event",
            Self::Other(value) => value,
        }
    }
}

impl Serialize for StreamEventType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for StreamEventType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "system" => Self::System,
            "assistant" => Self::Assistant,
            "user" => Self::User,
            "result" => Self::Result,
            "hook" => Self::Hook,
            "prompt_suggestion" => Self::PromptSuggestion,
            "rate_limit_event" => Self::RateLimitEvent,
            _ => Self::Other(value),
        })
    }
}

/// Parse newline-delimited stream-json output.
///
/// Empty lines are ignored because some terminals/scripts add trailing
/// newlines around the stream.
///
/// # Errors
///
/// Returns the first JSON parsing error with its original `serde_json`
/// location.
pub fn parse_stream_json(stdout: &str) -> Result<Vec<StreamEvent>, serde_json::Error> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

/// Stream of Claude Code `stream-json` events.
///
/// This type represents both live process output and already-collected output.
/// Callers consume it through the standard [`Stream`] trait.
pub struct StreamOutput {
    inner: Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>>,
}

impl StreamOutput {
    pub(crate) fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<StreamEvent, Error>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Create stream output from already-collected events.
    #[must_use]
    pub fn from_events(events: Vec<StreamEvent>) -> Self {
        Self::from_stream(stream::iter(events.into_iter().map(Ok)))
    }
}

impl fmt::Debug for StreamOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamOutput").finish_non_exhaustive()
    }
}

impl Stream for StreamOutput {
    type Item = Result<StreamEvent, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

impl TryFrom<ProcessOutput> for StreamOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        let stdout = String::from_utf8_lossy(&output.stdout);
        match parse_stream_json(&stdout) {
            Ok(events) => {
                if output.status.success() || !events.is_empty() {
                    Ok(Self::from_events(events))
                } else {
                    Err(Error::Cli {
                        exit_code: output.status.code(),
                        stdout: stdout.into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            }
            Err(err) => {
                if output.status.success() {
                    Err(Error::Json(err))
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
