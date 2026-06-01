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
//! `--output-format json` emits a JSON result object containing:
//!
//! ```jsonc
//! {
//!   "sessionId": "<uuid>",          // camelCase — feeds iter's session Factors
//!   "response":  "<final text>",    // the final assistant message
//!   "finishReason": "stop" | ...    // stop / finish reason
//! }
//! ```
//!
//! Only `sessionId` is pinned by this contract; every other field name is
//! read defensively via `serde_json::Value::get` because the exact shape
//! beyond `sessionId` is not guaranteed across CLI revisions.
//!
//! Field → conclusion chain: parse the JSON (whole-stream object first, then
//! a streaming `result` event as a fallback). A present object means the CLI
//! ran a turn; an `error`/refusal field — or a token-limit pattern in the
//! text — turns it into a [`GrokError`]. No JSON object plus a non-zero exit
//! means the process never produced a result; a terminating signal is
//! surfaced as such.

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
    /// A normal stop (`finishReason: "stop"` / `"end_turn"`).
    Stop,
    /// Any other finish-reason string the CLI emits.
    Other(String),
    /// No finish reason was reported.
    Unknown,
}

impl GrokStopReason {
    fn parse(reason: Option<&str>) -> Self {
        match reason {
            Some("stop" | "end_turn") => Self::Stop,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Unknown,
        }
    }
}

/// CLI-shaped result of a successful Grok Build headless run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct GrokResult {
    /// `sessionId` from the terminal object (feeds iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// Final assistant message, when reported.
    pub(crate) final_message: Option<String>,
    /// Parsed finish reason.
    pub(crate) stop_reason: GrokStopReason,
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

/// Read the final response text. Field name beyond `sessionId` is not pinned,
/// so try the most likely candidates defensively.
fn final_message_of(obj: &Value) -> Option<String> {
    for key in ["response", "result", "message", "text", "output"] {
        if let Some(text) = obj.get(key).and_then(Value::as_str) {
            return Some(text.to_owned());
        }
    }
    None
}

/// Read the finish / stop reason, trying the likely field names.
fn stop_reason_of(obj: &Value) -> Option<String> {
    for key in ["finishReason", "finish_reason", "stopReason", "stop_reason"] {
        if let Some(reason) = obj.get(key).and_then(Value::as_str) {
            return Some(reason.to_owned());
        }
    }
    None
}

/// Detect an in-band error / refusal in the terminal object. Returns a
/// human-readable summary when the object indicates a failure.
fn reported_error_of(obj: &Value) -> Option<String> {
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
    // Whole-stream JSON object first; fall back to a streaming `result` event.
    let terminal = cli_json::single_object(&stdout)
        .or_else(|| cli_json::last_event_of_type(&stdout, "result"));

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let Some(value) = terminal else {
        // Never produced a terminal object → never ran a turn.
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

    if let Some(message) = reported_error_of(&value) {
        // A failing result: refine into token-limit when the text says so.
        if let Some(detail) = detect_token_limit(&message).or_else(|| detect_token_limit(&stdout)) {
            return Err(GrokError::TokenLimit(detail));
        }
        return Err(GrokError::Reported { message, exit_code });
    }

    Ok(GrokResult {
        session_id: session_id_of(&value),
        final_message: final_message_of(&value),
        stop_reason: GrokStopReason::parse(stop_reason_of(&value).as_deref()),
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
    fn parses_successful_result_with_session_and_message() {
        let json = r#"{"sessionId":"sess-1","response":"done","finishReason":"stop"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.final_message.as_deref(), Some("done"));
        assert_eq!(res.stop_reason, GrokStopReason::Stop);
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
