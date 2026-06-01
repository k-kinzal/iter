//! `CursorCommand` — the **Command level** for the Cursor `cursor-agent`
//! CLI's print mode.
//!
//! Owns the print-mode argv (requesting `--output-format json`) and parses
//! the CLI's complete output into a CLI-shaped [`CursorResult`] or a
//! CLI-shaped [`CursorError`] hierarchy. Nothing here is iter-domain —
//! projecting these onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (`cursor-agent --print --output-format json`, 2026.03)
//!
//! On **success** the stream ends with a single terminal `result` record:
//!
//! ```jsonc
//! {
//!   "type": "result",
//!   "subtype": "success",
//!   "is_error": false,          // hard-coded — DO NOT trust as a signal
//!   "result": "<final assistant message>",
//!   "session_id": "<uuid>",
//!   "request_id": "<uuid>",
//!   "usage": { "input_tokens": 0, "output_tokens": 0, "num_turns": 0 },
//!   "duration_ms": 0
//! }
//! ```
//!
//! On **failure** there is **no** terminal `result` record at all: the stream
//! EOFs early and the error goes to stderr (occasionally as a `type:"error"`
//! record on stdout).
//!
//! Field → conclusion chain:
//!
//! * *did it run / success* = **presence of the terminal `result` record**.
//!   `is_error` is hard-coded `false` in this CLI revision, so it carries no
//!   information and is deliberately ignored.
//! * *why (on failure)* = a `type:"error"` record's message if one was
//!   emitted, else the tail of stderr.
//!
//! exit-code surface: `0` success · `1` catch-all · `2` below-minVersion ·
//! `127` launch · `128+n` signal.

use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Number of trailing stderr bytes carried in a no-result failure. Bounded so
/// a runaway child cannot bloat the error message; large enough to capture a
/// typical single-line CLI diagnostic.
const STDERR_TAIL_LIMIT: usize = 2_000;

/// Builds the Cursor `cursor-agent` print-mode argv.
pub(crate) struct CursorCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, appended after the managed flags so they
    /// can still override iter's defaults.
    pub(crate) args: &'a [String],
}

impl CursorCommand<'_> {
    /// Build the print-mode [`Command`]. `--print` makes `cursor-agent` emit
    /// a single response and exit; `--output-format json` makes the terminal
    /// `result` record machine-readable. The managed flags come first so the
    /// caller's `args` can still override them.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("--print");
        cmd.arg("--output-format").arg("json");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Token usage reported in the terminal `result` record's `usage` object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CursorUsage {
    /// `usage.input_tokens`, when reported.
    pub(crate) input_tokens: Option<u64>,
    /// `usage.output_tokens`, when reported.
    pub(crate) output_tokens: Option<u64>,
    /// `usage.num_turns`, when reported.
    pub(crate) num_turns: Option<u64>,
}

/// CLI-shaped result of a successful Cursor print run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct CursorResult {
    /// `session_id` from the terminal record (feeds iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// `request_id` from the terminal record.
    pub(crate) request_id: Option<String>,
    /// Final assistant message (`result`).
    pub(crate) final_message: Option<String>,
    /// Parsed `usage` object.
    pub(crate) usage: CursorUsage,
}

/// CLI-shaped error hierarchy for the Cursor `cursor-agent` CLI.
///
/// Deliberately omits `Cancelled` and `Launch` (I/O-level spawn failures):
/// those are owned by the shared spawn primitive / driver, not the Command.
#[derive(Debug, Error)]
pub(crate) enum CursorError {
    /// Context-window / token-limit detected in the output.
    #[error("cursor-agent hit the context/token limit: {0}")]
    TokenLimit(String),
    /// The process was terminated by a signal before producing a result.
    #[error("cursor-agent was terminated by signal {0}")]
    Signal(i32),
    /// Exit code `2`: the installed `cursor-agent` is below the minimum
    /// supported version. The agent never ran a turn.
    #[error("cursor-agent is below the minimum supported version (exit 2)")]
    BelowMinVersion,
    /// The process exited without ever producing a terminal `result` record.
    /// Carries the exit code (when one was produced) and the tail of stderr
    /// (or a `type:"error"` record's message) explaining why.
    #[error("cursor-agent produced no terminal result (exit code {exit_code:?}): {detail}")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
        /// Short diagnostic: a `type:"error"` record message or stderr tail.
        detail: String,
    },
}

/// Parse Cursor's complete print-mode output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<CursorResult, CursorError> {
    let stdout = output.stdout_str();
    // The success contract is "the whole stream is one JSON document", but a
    // streaming revision may precede the terminal record with progress lines;
    // fall back to scanning for the last `result` event so both shapes parse.
    let terminal = cli_json::single_object(&stdout)
        .filter(is_result_record)
        .or_else(|| cli_json::last_event_of_type(&stdout, "result"));

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let Some(value) = terminal else {
        // No terminal `result` record → cursor-agent never ran a turn.
        return Err(classify_failure(output, &stdout, exit_code));
    };

    // Success: a terminal `result` record is present. `is_error` is
    // hard-coded `false` in this CLI revision, so it is intentionally not
    // consulted. Read fields defensively via `.get()`.
    Ok(CursorResult {
        session_id: string_field(&value, "session_id"),
        request_id: string_field(&value, "request_id"),
        final_message: string_field(&value, "result"),
        usage: parse_usage(value.get("usage")),
    })
}

/// Build the failure error for a run that produced no terminal `result`.
///
/// Refinement order: token-limit (in stdout or stderr) → signal → exit `2`
/// below-min-version → a generic no-result error carrying the exit code and a
/// short diagnostic (a `type:"error"` record message, else the stderr tail).
fn classify_failure(output: &CommandOutput, stdout: &str, exit_code: Option<i32>) -> CursorError {
    let stderr = output.stderr_str();
    if let Some(detail) = detect_token_limit(stdout).or_else(|| detect_token_limit(&stderr)) {
        return CursorError::TokenLimit(detail);
    }
    if let RawExit::Signal(sig) = output.exit {
        return CursorError::Signal(sig);
    }
    if matches!(output.exit, RawExit::Code(2)) {
        return CursorError::BelowMinVersion;
    }
    CursorError::NoResult {
        exit_code,
        detail: failure_detail(stdout, &stderr),
    }
}

/// Short diagnostic for a no-result failure: prefer a `type:"error"` record's
/// message from stdout, otherwise fall back to the tail of stderr.
fn failure_detail(stdout: &str, stderr: &str) -> String {
    if let Some(event) = cli_json::last_event_of_type(stdout, "error")
        && let Some(message) = string_field(&event, "message").or_else(|| string_field(&event, "error"))
    {
        return message;
    }
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return "no error output".to_owned();
    }
    let start = trimmed.len().saturating_sub(STDERR_TAIL_LIMIT);
    let start = (start..=trimmed.len())
        .find(|&i| trimmed.is_char_boundary(i))
        .unwrap_or(trimmed.len());
    trimmed[start..].to_owned()
}

/// `true` when `value` is an object whose `type` field equals `"result"`.
fn is_result_record(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("result")
}

/// Read a string field from a JSON object, defensively.
fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Parse the optional `usage` object, tolerating missing fields.
fn parse_usage(value: Option<&Value>) -> CursorUsage {
    let Some(value) = value else {
        return CursorUsage::default();
    };
    CursorUsage {
        input_tokens: value.get("input_tokens").and_then(Value::as_u64),
        output_tokens: value.get("output_tokens").and_then(Value::as_u64),
        num_turns: value.get("num_turns").and_then(Value::as_u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str, stderr: &str, exit: RawExit) -> CommandOutput {
        CommandOutput {
            exit,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_successful_result_with_session_request_and_usage() {
        let json = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"sess-1","request_id":"req-9","usage":{"input_tokens":12,"output_tokens":34,"num_turns":2},"duration_ms":99}"#;
        let res = interpret(&output(json, "", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.request_id.as_deref(), Some("req-9"));
        assert_eq!(res.final_message.as_deref(), Some("done"));
        assert_eq!(res.usage.input_tokens, Some(12));
        assert_eq!(res.usage.output_tokens, Some(34));
        assert_eq!(res.usage.num_turns, Some(2));
    }

    #[test]
    fn is_error_true_is_ignored_when_result_present() {
        // `is_error` is hard-coded and must NOT flip a present result to Err.
        let json = r#"{"type":"result","subtype":"success","is_error":true,"session_id":"s"}"#;
        let res = interpret(&output(json, "", RawExit::Code(0))).expect("present result is success");
        assert_eq!(res.session_id.as_deref(), Some("s"));
    }

    #[test]
    fn finds_terminal_result_after_progress_lines() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"s2\"}\n",
        );
        let res = interpret(&output(stream, "", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn no_result_on_nonzero_exit_carries_stderr_tail() {
        let err = interpret(&output("", "fatal: boom\n", RawExit::Code(1))).expect_err("err");
        match err {
            CursorError::NoResult { exit_code, detail } => {
                assert_eq!(exit_code, Some(1));
                assert!(detail.contains("fatal: boom"), "got {detail:?}");
            }
            other => panic!("expected NoResult, got {other:?}"),
        }
    }

    #[test]
    fn no_result_prefers_error_record_message() {
        let stdout = "{\"type\":\"error\",\"message\":\"auth required\"}\n";
        let err = interpret(&output(stdout, "noise", RawExit::Code(1))).expect_err("err");
        match err {
            CursorError::NoResult { detail, .. } => assert_eq!(detail, "auth required"),
            other => panic!("expected NoResult, got {other:?}"),
        }
    }

    #[test]
    fn exit_two_maps_to_below_min_version() {
        let err = interpret(&output("", "needs upgrade", RawExit::Code(2))).expect_err("err");
        assert!(matches!(err, CursorError::BelowMinVersion));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", "", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, CursorError::Signal(9)));
    }

    #[test]
    fn token_limit_in_stderr_is_detected() {
        let err =
            interpret(&output("", "Error: context window exceeded\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CursorError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_in_stdout_is_detected() {
        let err = interpret(&output("too many tokens for this model\n", "", RawExit::Code(1)))
            .expect_err("err");
        assert!(matches!(err, CursorError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_takes_precedence_over_exit_two() {
        // A below-min-version exit that also mentions a token limit refines to
        // the router-relevant TokenLimit, not BelowMinVersion.
        let err = interpret(&output("", "context window exceeded", RawExit::Code(2)))
            .expect_err("err");
        assert!(matches!(err, CursorError::TokenLimit(_)));
    }

    #[test]
    fn empty_stderr_yields_placeholder_detail() {
        let err = interpret(&output("", "", RawExit::Code(1))).expect_err("err");
        match err {
            CursorError::NoResult { detail, .. } => assert_eq!(detail, "no error output"),
            other => panic!("expected NoResult, got {other:?}"),
        }
    }
}
