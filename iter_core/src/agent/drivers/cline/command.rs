//! `ClineCommand` — the **Command level** for the Cline CLI's oneshot mode.
//!
//! Owns the oneshot argv (requesting `--json`) and parses the CLI's complete
//! output into a CLI-shaped [`ClineResult`] or a CLI-shaped [`ClineError`]
//! hierarchy. Nothing here is iter-domain — projecting these onto
//! [`AgentRun`](crate::agent::AgentRun) / [`AgentError`](crate::agent::AgentError)
//! is the driver's job (see the `From` impls in `mod.rs`).
//!
//! # Output contract (Cline CLI, `--oneshot --json`)
//!
//! The stream is NDJSON / JSON-lines: any number of progress / error event
//! records followed by a terminal record. The records iter keys off:
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
//! # Exit-code surface
//!
//! `0` completed · `1` not-completed / threw / timeout / bad-args. A raw
//! Commander argument-parse error may leak exit `0`, so the exit code is a
//! weak signal: the terminal `run_result` record is authoritative, the exit
//! code is only consulted when no record was produced.

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Builds the Cline oneshot-mode argv.
pub(crate) struct ClineCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, appended after the managed flags so the
    /// caller can still override them.
    pub(crate) args: &'a [String],
}

impl ClineCommand<'_> {
    /// Build the oneshot-mode [`Command`]. `--oneshot` runs a single turn and
    /// exits; `--json` makes the terminal `run_result` record machine-readable.
    /// The managed flags come first so the caller's `args` can override them.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("--oneshot");
        cmd.arg("--json");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Finish reason reported in the terminal `run_result` record's `finishReason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClineFinishReason {
    /// `finishReason: "completed"`.
    Completed,
    /// Any other finish-reason string the CLI emits.
    Other(String),
}

impl ClineFinishReason {
    fn parse(finish_reason: Option<&str>) -> Self {
        match finish_reason {
            Some("completed") => Self::Completed,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Other(String::new()),
        }
    }
}

/// CLI-shaped result of a successful Cline oneshot run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct ClineResult {
    /// `sessionId` from the terminal record, when present (feeds iter's
    /// session Factors).
    pub(crate) session_id: Option<String>,
    /// Parsed `finishReason`.
    pub(crate) finish_reason: ClineFinishReason,
    /// Final assistant message (`message`), when present.
    pub(crate) final_message: Option<String>,
}

/// CLI-shaped error hierarchy for Cline.
#[derive(Debug, Error)]
pub(crate) enum ClineError {
    /// Context-window / token-limit detected in the output.
    #[error("cline hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A terminal `run_result` record whose `finishReason` was not
    /// `completed`.
    #[error("cline run did not complete (finishReason `{finish_reason}`)")]
    NotCompleted {
        /// The `finishReason` of the failing record.
        finish_reason: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// A `run_aborted` record, or an `error` event, surfaced before any
    /// terminal `run_result`.
    #[error("cline reported a failure event: {message}")]
    Reported {
        /// Short human-readable summary read from the event.
        message: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("cline was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal `run_result`
    /// record.
    #[error("cline produced no run_result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Raw terminal `run_result` record, deserialized.
#[derive(Debug, Deserialize)]
struct RunResult {
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// `true` for a JSON object that is a `run_aborted` record.
fn is_aborted(obj: &serde_json::Map<String, Value>) -> bool {
    type_or_kind(obj) == Some("run_aborted")
}

/// `true` for a JSON object that is an `error` event record.
fn is_error_event(obj: &serde_json::Map<String, Value>) -> bool {
    type_or_kind(obj) == Some("error")
}

/// Read the discriminator field (`type`, falling back to `kind`) off an object.
fn type_or_kind(obj: &serde_json::Map<String, Value>) -> Option<&str> {
    obj.get("type")
        .or_else(|| obj.get("kind"))
        .and_then(Value::as_str)
}

/// Extract a human-readable summary from a failure event, defensively.
fn event_message(value: &Value) -> String {
    value
        .get("message")
        .or_else(|| value.get("reason"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default()
}

/// Parse Cline's complete oneshot output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<ClineResult, ClineError> {
    let stdout = output.stdout_str();

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    // The terminal `run_result` record is authoritative for *did it run*.
    if let Some(value) = cli_json::last_event_of_type(&stdout, "run_result") {
        let record: RunResult = serde_json::from_value(value).unwrap_or(RunResult {
            finish_reason: None,
            session_id: None,
            message: None,
        });
        let finish_reason = ClineFinishReason::parse(record.finish_reason.as_deref());
        if finish_reason == ClineFinishReason::Completed {
            return Ok(ClineResult {
                session_id: record.session_id,
                finish_reason,
                final_message: record.message,
            });
        }
        // Ran a turn but did not complete — refine into token-limit when the
        // stream text says so, otherwise report the finish reason.
        if let Some(detail) = record
            .message
            .as_deref()
            .and_then(detect_token_limit)
            .or_else(|| detect_token_limit(&stdout))
        {
            return Err(ClineError::TokenLimit(detail));
        }
        return Err(ClineError::NotCompleted {
            finish_reason: record.finish_reason.unwrap_or_default(),
            exit_code,
        });
    }

    // No terminal record. Run token-limit detection over the stream first so a
    // context-window failure is classified before the generic event paths.
    if let Some(detail) = detect_token_limit(&stdout) {
        return Err(ClineError::TokenLimit(detail));
    }
    let stderr = output.stderr_str();
    if let Some(detail) = detect_token_limit(&stderr) {
        return Err(ClineError::TokenLimit(detail));
    }

    // A `run_aborted` record or an `error` event explains the failure.
    if let Some(value) = cli_json::last_event_matching(&stdout, is_aborted)
        .or_else(|| cli_json::first_event_matching(&stdout, is_error_event))
    {
        return Err(ClineError::Reported {
            message: event_message(&value),
            exit_code,
        });
    }

    // Nothing in-band: a signal is process-level termination; any other
    // disposition is a no-result failure carrying whatever exit code surfaced.
    if let RawExit::Signal(sig) = output.exit {
        return Err(ClineError::Signal(sig));
    }
    Err(ClineError::NoResult { exit_code })
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
    fn parses_completed_run_with_session_and_message() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"run_result\",\"finishReason\":\"completed\",\"sessionId\":\"sess-1\",\"message\":\"done\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.finish_reason, ClineFinishReason::Completed);
        assert_eq!(res.final_message.as_deref(), Some("done"));
    }

    #[test]
    fn completed_without_session_is_ok() {
        let stream = "{\"type\":\"run_result\",\"finishReason\":\"completed\"}\n";
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert!(res.session_id.is_none());
        assert_eq!(res.finish_reason, ClineFinishReason::Completed);
    }

    #[test]
    fn non_completed_finish_reason_maps_to_not_completed() {
        let stream = "{\"type\":\"run_result\",\"finishReason\":\"max_turns\"}\n";
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            ClineError::NotCompleted { ref finish_reason, exit_code: Some(1) }
                if finish_reason == "max_turns"
        ));
    }

    #[test]
    fn token_limit_in_run_result_message_is_detected() {
        let stream = "{\"type\":\"run_result\",\"finishReason\":\"error\",\"message\":\"context window exceeded\"}\n";
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClineError::TokenLimit(_)));
    }

    #[test]
    fn run_aborted_maps_to_reported() {
        let stream = "{\"type\":\"run_aborted\",\"reason\":\"user cancelled\"}\n";
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            ClineError::Reported { ref message, exit_code: Some(1) }
                if message == "user cancelled"
        ));
    }

    #[test]
    fn error_event_maps_to_reported() {
        let stream = "{\"type\":\"error\",\"message\":\"boom\"}\n";
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            ClineError::Reported { ref message, .. } if message == "boom"
        ));
    }

    #[test]
    fn token_limit_without_result_is_detected() {
        let err =
            interpret(&output("fatal: too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClineError::TokenLimit(_)));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, ClineError::Signal(9)));
    }

    #[test]
    fn no_result_on_nonzero_exit() {
        let err = interpret(&output("garbage\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClineError::NoResult { exit_code: Some(1) }));
    }

    #[test]
    fn run_result_is_authoritative_over_earlier_error_event() {
        // An error event earlier in the stream must not override a terminal
        // `run_result: completed`.
        let stream = concat!(
            "{\"type\":\"error\",\"message\":\"transient\"}\n",
            "{\"type\":\"run_result\",\"finishReason\":\"completed\",\"sessionId\":\"s\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s"));
    }
}
