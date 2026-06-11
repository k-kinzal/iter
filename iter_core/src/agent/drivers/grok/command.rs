//! `GrokCommand` — the **Command level** for Grok Build's headless mode.
//!
//! Owns the headless argv (requesting `--output-format json`) and parses the
//! CLI's complete output into a CLI-shaped [`GrokResult`] or a CLI-shaped
//! [`GrokError`] hierarchy. Nothing here is iter-domain — projecting these
//! onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Headless argv
//!
//! ```text
//! grok -p "<prompt>" --always-approve --output-format json [-s <id>] [args...]
//! ```
//!
//! The prompt is the *value* of `-p` — delivered inline, not on stdin. The
//! managed flags (including `--output-format json`) come before the caller's
//! `args` so a caller can still override the output format downstream.
//!
//! # Output contract (Grok Build, `--output-format json`)
//!
//! Verified against `grok 0.2.45` headless (`grok -p … --output-format json`).
//! The whole stream is a single JSON result object:
//!
//! ```jsonc
//! {
//!   "text":       "<final assistant message>",
//!   "stopReason": "EndTurn",         // stop / finish reason
//!   "sessionId":  "<uuid>",          // camelCase — feeds iter's session Factors
//!   "requestId":  "<uuid>",          // server-side request id for this turn
//!   "thought":    "<reasoning text>" // present only when reasoning is shown
//! }
//! ```
//!
//! On failure Grok emits an error object instead — `{"type":"error",
//! "message":"…"}` — so the `type` discriminator must be checked before the
//! payload is read as a success. The `streaming-json` format terminates with
//! a `{"type":"end", …}` event carrying the same `stopReason`/`sessionId`/
//! `requestId` metadata (the `text` is delivered incrementally in `text`
//! events), which the fallback below also accepts.
//!
//! **No usage/cost in this revision.** `grok 0.2.45` reports *no* token-count
//! or cost fields in the headless JSON object (confirmed against the shipped
//! binary and `~/.grok/docs/user-guide/14-headless-mode.md`). [`GrokUsage`]
//! therefore parses such fields *defensively* — tolerating the plausible
//! `camelCase`/`snake_case` names a future revision (or an alternate model
//! path) might use — and yields an empty usage when, as today, none are
//! present, so the values are captured rather than silently discarded if Grok
//! ever starts emitting them. See [`GrokUsage`] for the tolerated field names.
//!
//! Only `sessionId` is pinned by this contract; every other field name is
//! read defensively via `serde_json::Value::get` because the exact shape
//! beyond `sessionId` is not guaranteed across CLI revisions.
//!
//! Field → conclusion chain: parse the JSON (whole-stream object first, then
//! a streaming `end`/`result` event as a fallback). A `type:"error"` object,
//! an `error`/refusal field, or a token-limit pattern in the text turns it
//! into a [`GrokError`]; any other present object means the CLI ran a turn.
//! No JSON object plus a non-zero exit means the process never produced a
//! result; a terminating signal is surfaced as such.

use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use crate::prompt::Prompt;
use std::path::Path;

/// Builds the Grok Build headless argv.
pub(crate) struct GrokCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// The prompt — delivered inline as the value of `-p`.
    pub(crate) prompt: &'a Prompt,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
    /// Resolved session id, when session persistence is configured.
    pub(crate) session_id: Option<&'a str>,
}

impl GrokCommand<'_> {
    /// Build the headless [`Command`]. Emits `--output-format json` (before
    /// the caller's `args`, so it can still be overridden) so the terminal
    /// result object is machine-readable.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        // `-p <prompt>` is the headless trigger: the prompt is the *value* of
        // the flag, delivered inline (no stdin).
        cmd.arg("-p").arg(self.prompt.as_str());
        cmd.arg("--always-approve");
        cmd.arg("--output-format").arg("json");
        if let Some(sid) = self.session_id {
            cmd.arg("-s").arg(sid);
        }
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Stop / finish reason reported in the terminal result object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrokStopReason {
    /// A normal stop (`stopReason: "EndTurn"`; legacy `finishReason: "stop"` /
    /// `"end_turn"`).
    Stop,
    /// Any other finish-reason string the CLI emits.
    Other(String),
    /// No finish reason was reported.
    Unknown,
}

impl GrokStopReason {
    fn parse(reason: Option<&str>) -> Self {
        match reason {
            // `EndTurn` is the value `grok 0.2.45` reports on a normal stop;
            // the snake/lower forms are kept for cross-revision tolerance.
            Some("EndTurn" | "stop" | "end_turn") => Self::Stop,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Unknown,
        }
    }
}

/// Token-usage / cost reported in the terminal object.
///
/// `grok 0.2.45` headless JSON reports **none** of these, so every field is
/// normally `None`; this type exists so that the values are captured rather
/// than silently dropped if a future Grok revision (or a model path that does
/// surface accounting) starts reporting them. Each field tolerates the
/// plausible `camelCase`/`snake_case` spellings; the `usage`-like nesting and
/// the top-level object are both searched (see [`parse_usage`]):
///
/// * `input_tokens` ← `input_tokens` / `inputTokens` / `prompt_tokens` /
///   `promptTokens`
/// * `output_tokens` ← `output_tokens` / `outputTokens` /
///   `completion_tokens` / `completionTokens`
/// * `total_tokens` ← `total_tokens` / `totalTokens`
/// * `total_cost_usd` ← `total_cost_usd` / `totalCostUsd` / `cost_usd` /
///   `costUsd` / `cost`
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct GrokUsage {
    /// Prompt / input token count, when reported.
    pub(crate) input_tokens: Option<u64>,
    /// Completion / output token count, when reported.
    pub(crate) output_tokens: Option<u64>,
    /// Total token count, when reported.
    pub(crate) total_tokens: Option<u64>,
    /// Reported run cost in USD, when reported.
    pub(crate) total_cost_usd: Option<f64>,
}

/// CLI-shaped result of a successful Grok Build headless run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design. iter's
// domain model (`AgentRun`) intentionally projects only `session_id`; the
// remaining fields — including `usage` — stay at this Command layer (read by
// this module's tests and reserved for future Factors) rather than being
// pushed into `AgentRun`, mirroring how the Cursor/Claude drivers keep their
// usage/cost at the Command level. See the `From`/projection note in `mod.rs`.
#[allow(dead_code)]
pub(crate) struct GrokResult {
    /// `sessionId` from the terminal object (feeds iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// `requestId` from the terminal object — the server-side id for the turn.
    pub(crate) request_id: Option<String>,
    /// Final assistant message (`text`), when reported.
    pub(crate) final_message: Option<String>,
    /// Reasoning text (`thought`), present only when reasoning is shown.
    pub(crate) thought: Option<String>,
    /// Parsed finish reason.
    pub(crate) stop_reason: GrokStopReason,
    /// Parsed token-usage / cost (empty on `grok 0.2.45`).
    pub(crate) usage: GrokUsage,
}

/// CLI-shaped error hierarchy for Grok Build.
#[derive(Debug, Error)]
pub(crate) enum GrokError {
    /// Context-window / token-limit detected in the output.
    #[error("grok hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A terminal result object that reported an error / refusal.
    #[error("grok reported an error result: {message}")]
    Reported {
        /// Human-readable summary of the reported failure.
        message: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("grok was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal result object.
    #[error("grok produced no terminal result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Read the `sessionId` field (camelCase per the contract).
fn session_id_of(obj: &Value) -> Option<String> {
    obj.get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Read the `requestId` field (camelCase per the verified contract).
fn request_id_of(obj: &Value) -> Option<String> {
    first_str(obj, &["requestId", "request_id"])
}

/// Read the reasoning text (`thought`), when reasoning is shown.
fn thought_of(obj: &Value) -> Option<String> {
    first_str(obj, &["thought", "reasoning"])
}

/// Read the final response text. `text` is the field `grok 0.2.45` emits; the
/// rest are kept as defensive fallbacks since the name is not pinned beyond
/// `sessionId`.
fn final_message_of(obj: &Value) -> Option<String> {
    first_str(obj, &["text", "response", "result", "message", "output"])
}

/// First present string among `keys`.
fn first_str(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

/// Parse token-usage / cost defensively (see [`GrokUsage`]). Both a
/// `usage`-like sub-object and the top-level object are searched, so a flat or
/// a nested report both resolve. Name specificity wins over scope: each field
/// tries its names most-canonical-first across *every* scope before falling
/// through to the next alias, so a generic `cost` nested under `usage` never
/// shadows a canonical `total_cost_usd` at the top level. For one *same* name
/// present in both scopes, the nested `usage` object is preferred (it is the
/// more specific accounting container). Yields an empty [`GrokUsage`] when
/// nothing is reported — the `grok 0.2.45` case.
fn parse_usage(root: &Value) -> GrokUsage {
    let nested = ["usage", "tokenUsage", "token_usage"]
        .iter()
        .find_map(|key| root.get(*key));
    let scopes: Vec<&Value> = nested.into_iter().chain(std::iter::once(root)).collect();
    let u64_of = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| scopes.iter().find_map(|s| s.get(*key).and_then(Value::as_u64)))
    };
    let f64_of = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| scopes.iter().find_map(|s| s.get(*key).and_then(Value::as_f64)))
    };
    GrokUsage {
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

/// Read the finish / stop reason. The verified `stopReason` is probed first,
/// ahead of the legacy `finishReason`, so a future object carrying both still
/// resolves to the canonical field.
fn stop_reason_of(obj: &Value) -> Option<String> {
    for key in ["stopReason", "stop_reason", "finishReason", "finish_reason"] {
        if let Some(reason) = obj.get(key).and_then(Value::as_str) {
            return Some(reason.to_owned());
        }
    }
    None
}

/// Detect an in-band error / refusal in the terminal object. Returns a
/// human-readable summary when the object indicates a failure.
fn reported_error_of(obj: &Value) -> Option<String> {
    // The verified failure shape: `{"type":"error","message":"…"}`. The
    // `type` discriminator must be honored before the object is read as a
    // success, otherwise the error `message` would be mistaken for a result.
    if obj.get("type").and_then(Value::as_str) == Some("error") {
        return Some(
            first_str(obj, &["message", "error"]).unwrap_or_else(|| "error".to_owned()),
        );
    }
    // An explicit `error` object/string.
    if let Some(error) = obj.get("error") {
        match error {
            Value::String(s) if !s.is_empty() => return Some(s.clone()),
            Value::Object(_) => {
                let msg = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| "error".to_owned(), str::to_owned);
                return Some(msg);
            }
            _ => {}
        }
    }
    // A boolean error flag (`isError` / `is_error`) paired with the message.
    let flagged = obj
        .get("isError")
        .or_else(|| obj.get("is_error"))
        .and_then(Value::as_bool)
        == Some(true);
    if flagged {
        let msg = final_message_of(obj).unwrap_or_else(|| "error".to_owned());
        return Some(msg);
    }
    None
}

/// Parse Grok Build's complete headless output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<GrokResult, GrokError> {
    let stdout = output.stdout_str();
    // Whole-stream JSON object first; then the `streaming-json` terminal
    // event (`type:"end"` in `grok 0.2.45`), then a legacy `result` event.
    let terminal = cli_json::single_object(&stdout)
        .or_else(|| cli_json::last_event_of_type(&stdout, "end"))
        .or_else(|| cli_json::last_event_of_type(&stdout, "result"));

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let Some(value) = terminal else {
        // Never produced a terminal object → never ran a turn. Here there is
        // no structured message to trust, so the whole stdout/stderr is scanned
        // for a token-limit pattern.
        if let RawExit::Signal(sig) = output.exit {
            return Err(GrokError::Signal(sig));
        }
        if let Some(detail) = detect_token_limit(&stdout) {
            return Err(GrokError::TokenLimit(detail));
        }
        let stderr = output.stderr_str();
        if let Some(detail) = detect_token_limit(&stderr) {
            return Err(GrokError::TokenLimit(detail));
        }
        return Err(GrokError::NoResult { exit_code });
    };
    // A terminal object *was* produced. A terminating signal that arrived after
    // the CLI already wrote its result is not a failure — the turn completed
    // and its output is usable — so the `Signal` variant deliberately applies
    // only to the no-terminal branch above (matches the Cursor driver).

    // An in-band failure: either the terminal object itself reports an error,
    // or — in `streaming-json` — a `type:"error"` event was emitted alongside
    // the success-looking terminal `end` event and must not be swallowed. Any
    // error event in the stream is treated as a turn failure (fail-safe);
    // `last_event_of_type` surfaces the final one when several are present
    // (e.g. transient retries before a fatal error), matching the Cursor driver.
    let reported = reported_error_of(&value).or_else(|| {
        cli_json::last_event_of_type(&stdout, "error").and_then(|e| reported_error_of(&e))
    });
    if let Some(message) = reported {
        // Refine into a token-limit only from the *authoritative* error
        // `message`, never the full stdout: streamed `text`/`thought` content
        // can legitimately mention "context window" and must not flip an
        // unrelated failure (auth, rate-limit) into a `TokenLimit`.
        if let Some(detail) = detect_token_limit(&message) {
            return Err(GrokError::TokenLimit(detail));
        }
        return Err(GrokError::Reported { message, exit_code });
    }

    Ok(GrokResult {
        session_id: session_id_of(&value),
        request_id: request_id_of(&value),
        final_message: final_message_of(&value),
        thought: thought_of(&value),
        stop_reason: GrokStopReason::parse(stop_reason_of(&value).as_deref()),
        usage: parse_usage(&value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str, exit: RawExit) -> CommandOutput {
        CommandOutput {
            exit,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn parses_verified_grok_0_2_45_result() {
        // The exact shape `grok 0.2.45 -p … --output-format json` emits: no
        // usage/cost fields, `EndTurn` stop reason, `text`/`requestId`/
        // `thought` present.
        let json = r#"{"text":"OK","stopReason":"EndTurn","sessionId":"019eb76b","requestId":"6229237d","thought":"thinking..."}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("019eb76b"));
        assert_eq!(res.request_id.as_deref(), Some("6229237d"));
        assert_eq!(res.final_message.as_deref(), Some("OK"));
        assert_eq!(res.thought.as_deref(), Some("thinking..."));
        assert_eq!(res.stop_reason, GrokStopReason::Stop);
        assert_eq!(
            res.usage,
            GrokUsage::default(),
            "0.2.45 reports no usage/cost",
        );
    }

    #[test]
    fn parses_legacy_field_names_defensively() {
        // Older/alternate field names must still resolve: `response` for the
        // message, `finishReason` for the stop reason.
        let json = r#"{"sessionId":"sess-1","response":"done","finishReason":"stop"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.final_message.as_deref(), Some("done"));
        assert_eq!(res.stop_reason, GrokStopReason::Stop);
    }

    #[test]
    fn parses_usage_cost_when_present_nested_or_flat() {
        // A realistic (hypothetical) future object that *does* report usage:
        // tokens nested under `usage`, cost flat at the top level. Mixed
        // camelCase/snake_case is tolerated.
        let json = r#"{
            "text":"done","stopReason":"EndTurn","sessionId":"s",
            "usage":{"inputTokens":120,"output_tokens":34,"total_tokens":154},
            "totalCostUsd":0.0123
        }"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_ne!(res.usage, GrokUsage::default());
        assert_eq!(res.usage.input_tokens, Some(120));
        assert_eq!(res.usage.output_tokens, Some(34));
        assert_eq!(res.usage.total_tokens, Some(154));
        assert_eq!(res.usage.total_cost_usd, Some(0.0123));
    }

    #[test]
    fn usage_is_empty_when_fields_absent() {
        // Missing-field tolerance: no usage object, no cost → empty, no panic.
        let json = r#"{"text":"hi","sessionId":"s"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.usage, GrokUsage::default());
    }

    #[test]
    fn end_turn_stop_reason_maps_to_stop() {
        let json = r#"{"text":"x","stopReason":"EndTurn","sessionId":"s"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.stop_reason, GrokStopReason::Stop);
    }

    #[test]
    fn type_error_object_maps_to_reported() {
        // The verified failure shape `{"type":"error","message":"…"}` must
        // become an error, not an `Ok` whose message is read as a result.
        let json = r#"{"type":"error","message":"Couldn't start session: auth"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, exit_code: Some(1) }
                if message == "Couldn't start session: auth"
        ));
    }

    #[test]
    fn streaming_end_event_is_used_as_fallback() {
        // `streaming-json` terminates with a `type:"end"` event carrying the
        // metadata (text arrives in prior `text` events).
        let stream = concat!(
            "{\"type\":\"text\",\"data\":\"O\"}\n",
            "{\"type\":\"end\",\"stopReason\":\"EndTurn\",\"sessionId\":\"s3\",\"requestId\":\"r3\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s3"));
        assert_eq!(res.request_id.as_deref(), Some("r3"));
        assert_eq!(res.stop_reason, GrokStopReason::Stop);
        // By design `final_message` is `None` on the streaming fallback: the
        // `end` event carries only metadata; text was delivered in `text`
        // events the headless `--output-format json` path never emits.
        assert_eq!(res.final_message, None);
    }

    #[test]
    fn streaming_error_before_end_event_maps_to_reported() {
        // A `type:"error"` event preceding the success-looking terminal `end`
        // event must surface as an error, not be masked by the `end` metadata.
        let stream = concat!(
            "{\"type\":\"error\",\"message\":\"rate limit exceeded\"}\n",
            "{\"type\":\"end\",\"stopReason\":\"EndTurn\",\"sessionId\":\"s3\"}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, .. } if message == "rate limit exceeded"
        ));
    }

    #[test]
    fn streaming_error_after_end_event_maps_to_reported() {
        // Fail-safe: an `error` event anywhere in the stream — even after the
        // terminal `end` — is treated as a turn failure, not silent success.
        let stream = concat!(
            "{\"type\":\"end\",\"stopReason\":\"EndTurn\",\"sessionId\":\"s3\"}\n",
            "{\"type\":\"error\",\"message\":\"post-turn failure\"}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, .. } if message == "post-turn failure"
        ));
    }

    #[test]
    fn non_token_error_with_token_phrase_in_stdout_stays_reported() {
        // A non-token failure must NOT be reclassified as a token limit just
        // because streamed `text` content happens to mention a token phrase.
        // The terminal `end` event makes the reported-error branch reachable;
        // refinement there reads the authoritative error `message` only, never
        // the full stdout (which here contains the "context window" phrase).
        let stream = concat!(
            "{\"type\":\"text\",\"data\":\"the context window matters here\"}\n",
            "{\"type\":\"error\",\"message\":\"authentication failed\"}\n",
            "{\"type\":\"end\",\"stopReason\":\"EndTurn\",\"sessionId\":\"s\"}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(
            matches!(err, GrokError::Reported { ref message, .. } if message == "authentication failed"),
            "got {err:?}",
        );
    }

    #[test]
    fn error_flag_with_text_field_maps_to_reported() {
        // The `isError` flag path reads the message from `text` (the verified
        // field) when present, mirroring the success-path field priority.
        let json = r#"{"sessionId":"s","isError":true,"text":"refused: policy"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, .. } if message == "refused: policy"
        ));
    }

    #[test]
    fn canonical_cost_name_beats_generic_nested_alias() {
        // Name specificity wins over scope: a generic `cost` nested under
        // `usage` must not shadow a canonical `total_cost_usd` at the top level.
        let json = r#"{"text":"x","sessionId":"s","usage":{"cost":99},"total_cost_usd":0.05}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.usage.total_cost_usd, Some(0.05));
    }

    #[test]
    fn streaming_result_event_is_used_as_fallback() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"result\",\"sessionId\":\"s2\",\"response\":\"hi\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s2"));
        assert_eq!(res.final_message.as_deref(), Some("hi"));
    }

    #[test]
    fn error_object_maps_to_reported() {
        let json = r#"{"sessionId":"s","error":{"message":"boom"}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, exit_code: Some(1) } if message == "boom"
        ));
    }

    #[test]
    fn error_flag_maps_to_reported() {
        let json = r#"{"sessionId":"s","isError":true,"response":"refused"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            GrokError::Reported { ref message, .. } if message == "refused"
        ));
    }

    #[test]
    fn token_limit_in_error_result_is_detected() {
        let json = r#"{"error":"Error: context window exceeded"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GrokError::TokenLimit(_)));
    }

    #[test]
    fn no_terminal_result_on_nonzero_exit() {
        let err = interpret(&output("garbage\n", RawExit::Code(2))).expect_err("err");
        assert!(matches!(err, GrokError::NoResult { exit_code: Some(2) }));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, GrokError::Signal(9)));
    }

    #[test]
    fn token_limit_without_result_object_is_detected() {
        let err =
            interpret(&output("fatal: too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GrokError::TokenLimit(_)));
    }
}
