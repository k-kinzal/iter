//! `OpenCodeCommand` — the **Command level** for `OpenCode`'s print mode.
//!
//! Owns the print-mode argv (`opencode run … --format json`) and parses the
//! CLI's complete output into a CLI-shaped [`OpenCodeResult`] or a CLI-shaped
//! [`OpenCodeError`] hierarchy. Nothing here is iter-domain — projecting these
//! onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (`OpenCode`, `--format json`) — the exit code lies
//!
//! `OpenCode` is one of the exit-0-but-failed CLIs: **the process exit code is
//! not the verdict**. The CLI exits `0` when it reaches an idle state with no
//! synchronous `result.error` — *including* auth / 429 / token-limit failures
//! (it either exits `0` or hangs). It exits `1` only on a pre-flight /
//! validation failure or a synchronous `result.error`.
//!
//! The verdict therefore lives in the output stream. A failure surfaces a
//! `session.error` event (a `result.error` may also appear on the synchronous
//! exit-1 path); these carry the failure message but **do not** set the exit
//! code:
//!
//! ```jsonc
//! { "type": "session.error", "error": { "message": "…" } }
//! ```
//!
//! A clean run surfaces a session record that reached `idle`:
//!
//! ```jsonc
//! { "type": "session", "id": "<id>", "status": "idle" }
//! ```
//!
//! Field → conclusion chain:
//!
//! * *did it run* = the session reached `idle`.
//! * *success/fail* = **presence of a `session.error` (or `result.error`)
//!   event** in the output — NOT the exit code.
//! * *why* = the error message (note: exit `0` even on failure).
//!
//! When an error event is found the message is run through
//! [`detect_token_limit`] (and the whole stream as a fallback) so a
//! context/token-limit failure refines into [`OpenCodeError::TokenLimit`];
//! otherwise it becomes [`OpenCodeError::Failed`], carrying the exit code only
//! when the process actually exited non-zero. With no error event the run is
//! `Ok` and any session id is read from a session record. A terminating signal
//! with no error event maps to [`OpenCodeError::Signal`].

use serde::Deserialize;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Builds the `OpenCode` print-mode argv.
pub(crate) struct OpenCodeCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, inserted between the `run` subcommand and
    /// the managed `--format json` flag so a caller can still override the
    /// format downstream.
    pub(crate) args: &'a [String],
    /// The prompt, delivered as the trailing positional argument.
    pub(crate) prompt: &'a str,
}

impl OpenCodeCommand<'_> {
    /// Build the print-mode [`Command`]. Requests `--format json` so the
    /// stream is machine-readable. The prompt is the final positional
    /// argument (`OpenCode`'s `run` takes the message positionally).
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("run");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd.arg("--format").arg("json");
        cmd.arg(self.prompt);
        cmd
    }
}

/// CLI-shaped result of a successful `OpenCode` print run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct OpenCodeResult {
    /// Session id read from a session record, when one is present (feeds
    /// iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// Final assistant message, when one could be recovered from the stream.
    pub(crate) final_message: Option<String>,
}

/// CLI-shaped error hierarchy for `OpenCode`.
///
/// No `Cancelled` / `Launch` variants live here — those are spawn-level
/// concerns owned by [`SpawnError`](crate::agent::process::SpawnError) and the
/// driver, not the output parser.
#[derive(Debug, Error)]
pub(crate) enum OpenCodeError {
    /// Context-window / token-limit detected in an error event's message (or
    /// the surrounding stream). Router-relevant: the Adapter maps this to
    /// [`AgentError::TokenLimit`].
    ///
    /// [`AgentError::TokenLimit`]: crate::agent::AgentError::TokenLimit
    #[error("opencode hit the context/token limit: {0}")]
    TokenLimit(String),
    /// An in-band `session.error` / `result.error` event was present in the
    /// output. This is the authoritative failure signal — the process may
    /// have exited `0`. `code` is the process exit code only when the process
    /// actually exited non-zero (the synchronous `result.error` path);
    /// `None` for the exit-0-but-failed path.
    #[error("opencode reported an error{}: {message}", match .code { Some(c) => format!(" (exit code {c})"), None => String::new() })]
    Failed {
        /// Process exit code, but only when the process exited non-zero.
        code: Option<i32>,
        /// The error message recovered from the event.
        message: String,
    },
    /// The process was terminated by a signal and produced no error event.
    #[error("opencode was terminated by signal {0}")]
    Signal(i32),
}

/// Raw error event (`session.error` / `result.error`), deserialized
/// defensively. `OpenCode` nests the human-readable text under `error.message`,
/// but tolerates a flat top-level `message` too.
#[derive(Debug, Deserialize)]
struct ErrorEvent {
    #[serde(default)]
    error: Option<ErrorBody>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
}

impl ErrorEvent {
    /// Best-effort extraction of the human-readable failure text.
    fn message(self) -> String {
        self.error
            .and_then(|body| body.message)
            .or(self.message)
            .unwrap_or_default()
    }
}

/// Match a JSON object's `type` / `kind` marker against `marker`.
fn is_type(obj: &serde_json::Map<String, serde_json::Value>, marker: &str) -> bool {
    obj.get("type")
        .or_else(|| obj.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some(marker)
}

/// `true` when the object is an error record (`session.error` or
/// `result.error`).
fn is_error_event(obj: &serde_json::Map<String, serde_json::Value>) -> bool {
    is_type(obj, "session.error") || is_type(obj, "result.error")
}

/// Parse `OpenCode`'s complete print-mode output into a result or error.
///
/// The presence of an error event — regardless of the exit code — *is* the
/// failure signal. Only when no error event is found do we treat the run as a
/// success and read any session id from the stream.
pub(crate) fn interpret(output: &CommandOutput) -> Result<OpenCodeResult, OpenCodeError> {
    let stdout = output.stdout_str();

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    // Presence of an error event is authoritative. Look for it first as a
    // single-object stream, then as a line in a JSON-lines stream.
    let error_value = cli_json::single_object(&stdout)
        .filter(|v| v.as_object().is_some_and(is_error_event))
        .or_else(|| cli_json::first_event_matching(&stdout, is_error_event));

    if let Some(value) = error_value {
        let record: ErrorEvent = serde_json::from_value(value).unwrap_or(ErrorEvent {
            error: None,
            message: None,
        });
        let message = record.message();
        // Refine into TokenLimit when the message — or anywhere in the
        // stream — describes a context/token limit.
        if let Some(detail) = detect_token_limit(&message).or_else(|| detect_token_limit(&stdout)) {
            return Err(OpenCodeError::TokenLimit(detail));
        }
        // Carry the exit code only when the process actually exited non-zero
        // (the synchronous `result.error` path); the exit-0-but-failed path
        // reports `None`.
        let code = exit_code.filter(|&c| c != 0);
        let message = if message.is_empty() {
            "opencode reported an error event".to_owned()
        } else {
            message
        };
        return Err(OpenCodeError::Failed { code, message });
    }

    // No error event. A terminating signal with no in-band error is a
    // process-level termination.
    if let RawExit::Signal(sig) = output.exit {
        return Err(OpenCodeError::Signal(sig));
    }

    // A non-zero exit with NO in-band error event is a pre-flight /
    // validation failure that crashed before OpenCode could write a
    // `result.error` (exit 1 covers "pre-flight/validation/synchronous
    // result.error"). The exit-0-but-failed path is already handled above by
    // the error-event check, so trusting a non-zero exit here is sound — it
    // is the only signal a never-emitted-JSON crash leaves behind.
    if let RawExit::Code(code) = output.exit
        && code != 0
    {
        return Err(OpenCodeError::Failed {
            code: Some(code),
            message: format!("opencode exited with code {code} and no result event"),
        });
    }

    // Success: recover any session id / final message from the stream.
    Ok(OpenCodeResult {
        session_id: session_id_from_stream(&stdout),
        final_message: final_message_from_stream(&stdout),
    })
}

/// Best-effort session id lookup. Prefers a session record, falling back to a
/// top-level `sessionId` / `session_id` on any record.
fn session_id_from_stream(stdout: &str) -> Option<String> {
    let value = cli_json::single_object(stdout)
        .or_else(|| cli_json::last_event_matching(stdout, |obj| is_type(obj, "session")))
        .or_else(|| {
            cli_json::last_event_matching(stdout, |obj| {
                obj.get("id")
                    .or_else(|| obj.get("sessionId"))
                    .or_else(|| obj.get("session_id"))
                    .is_some()
            })
        })?;
    let obj = value.as_object()?;
    obj.get("id")
        .or_else(|| obj.get("sessionId"))
        .or_else(|| obj.get("session_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Best-effort final-message lookup from a session / result record's `text`
/// or `message` field. Purely informational at the Command level.
fn final_message_from_stream(stdout: &str) -> Option<String> {
    let value = cli_json::single_object(stdout)
        .or_else(|| cli_json::last_event_matching(stdout, |obj| is_type(obj, "result")))
        .or_else(|| cli_json::last_event_matching(stdout, |obj| is_type(obj, "session")))?;
    let obj = value.as_object()?;
    obj.get("text")
        .or_else(|| obj.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
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
    fn parses_successful_session_with_id() {
        let json = r#"{"type":"session","id":"sess-1","status":"idle","text":"done"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.final_message.as_deref(), Some("done"));
    }

    #[test]
    fn reads_session_id_from_jsonl_stream() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"session\",\"id\":\"s9\",\"status\":\"idle\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s9"));
    }

    #[test]
    fn empty_stream_on_clean_exit_is_ok_without_session() {
        let res = interpret(&output("", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id, None);
    }

    #[test]
    fn session_error_on_exit_zero_is_a_failure() {
        // The exit code lies: 0 even though the run failed.
        let json = r#"{"type":"session.error","error":{"message":"auth failed"}}"#;
        let err = interpret(&output(json, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            OpenCodeError::Failed { code: None, ref message } if message == "auth failed"
        ));
    }

    #[test]
    fn result_error_on_exit_one_carries_exit_code() {
        let json = r#"{"type":"result.error","error":{"message":"bad flag"}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            OpenCodeError::Failed { code: Some(1), ref message } if message == "bad flag"
        ));
    }

    #[test]
    fn error_event_is_found_in_jsonl_stream() {
        let stream = concat!(
            "{\"type\":\"session\",\"id\":\"s\",\"status\":\"running\"}\n",
            "{\"type\":\"session.error\",\"error\":{\"message\":\"boom\"}}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            OpenCodeError::Failed { code: None, ref message } if message == "boom"
        ));
    }

    #[test]
    fn token_limit_in_error_message_refines_to_token_limit() {
        let json =
            r#"{"type":"session.error","error":{"message":"context window exceeded for model"}}"#;
        let err = interpret(&output(json, RawExit::Code(0))).expect_err("err");
        assert!(matches!(err, OpenCodeError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_elsewhere_in_stream_refines_error() {
        let stream = concat!(
            "{\"type\":\"log\",\"text\":\"too many tokens in the request\"}\n",
            "{\"type\":\"session.error\",\"error\":{}}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(0))).expect_err("err");
        assert!(matches!(err, OpenCodeError::TokenLimit(_)));
    }

    #[test]
    fn flat_message_field_is_recovered() {
        let json = r#"{"type":"session.error","message":"flat failure"}"#;
        let err = interpret(&output(json, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            OpenCodeError::Failed { ref message, .. } if message == "flat failure"
        ));
    }

    #[test]
    fn empty_error_event_gets_placeholder_message() {
        let json = r#"{"type":"session.error"}"#;
        let err = interpret(&output(json, RawExit::Code(0))).expect_err("err");
        assert!(matches!(
            err,
            OpenCodeError::Failed { ref message, .. } if !message.is_empty()
        ));
    }

    #[test]
    fn signal_without_error_event_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, OpenCodeError::Signal(9)));
    }

    #[test]
    fn error_event_is_authoritative_over_signal() {
        // An in-band error wins even on a signal exit.
        let json = r#"{"type":"session.error","error":{"message":"nope"}}"#;
        let err = interpret(&output(json, RawExit::Signal(15))).expect_err("err");
        assert!(matches!(err, OpenCodeError::Failed { code: None, .. }));
    }

    #[test]
    fn nonzero_exit_without_error_event_is_a_failure() {
        // A pre-flight/validation crash exits 1 before writing any
        // `result.error` JSON; trusting the non-zero exit here is the only
        // signal left, and it must NOT be misread as a successful turn.
        let err = interpret(&output("", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, OpenCodeError::Failed { code: Some(1), .. }), "got {err:?}");
    }
}
