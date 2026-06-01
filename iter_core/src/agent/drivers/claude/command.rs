//! `ClaudeCodeCommand` — the **Command level** for Claude Code's print mode.
//!
//! Owns the print-mode argv (requesting `--output-format json`) and parses
//! the CLI's complete output into a CLI-shaped [`ClaudeCodeResult`] or a
//! CLI-shaped [`ClaudeCodeError`] hierarchy. Nothing here is iter-domain —
//! projecting these onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impls in `mod.rs`).
//!
//! # Output contract (Claude Code, `--output-format json`)
//!
//! The stream is a single JSON object — the terminal `result` record:
//!
//! ```jsonc
//! {
//!   "type": "result",
//!   "subtype": "success" | "error_max_turns" | "error_during_execution",
//!   "is_error": false,
//!   "result": "<final assistant message>",
//!   "session_id": "<uuid>",
//!   "num_turns": 3,
//!   "total_cost_usd": 0.0123,
//!   "usage": { ... }
//! }
//! ```
//!
//! Field → conclusion chain: *did it run* = the `result` object is present;
//! *success/fail* = `is_error`; *why* = `subtype`. A process that never
//! produced a `result` object (non-zero exit, signal, launch error) never
//! ran a turn.

use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Builds the Claude Code print-mode argv.
pub(crate) struct ClaudeCodeCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
    /// Resolved session id, when session persistence is configured.
    pub(crate) session_id: Option<&'a str>,
}

impl ClaudeCodeCommand<'_> {
    /// Build the print-mode [`Command`]. Requests `--output-format json` so
    /// the terminal `result` record is machine-readable; the managed flags
    /// come first so the caller's `args` can still override them.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("--print");
        cmd.arg("--output-format").arg("json");
        cmd.arg("--permission-mode").arg("bypassPermissions");
        if let Some(sid) = self.session_id {
            cmd.arg("--session-id").arg(sid);
        }
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Stop reason reported in the terminal `result` record's `subtype`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaudeStopReason {
    /// `subtype: "success"`.
    Success,
    /// `subtype: "error_max_turns"`.
    MaxTurns,
    /// `subtype: "error_during_execution"`.
    ErrorDuringExecution,
    /// Any other subtype string the CLI emits.
    Other(String),
}

impl ClaudeStopReason {
    fn parse(subtype: Option<&str>) -> Self {
        match subtype {
            Some("success") => Self::Success,
            Some("error_max_turns") => Self::MaxTurns,
            Some("error_during_execution") => Self::ErrorDuringExecution,
            Some(other) => Self::Other(other.to_owned()),
            None => Self::Other(String::new()),
        }
    }
}

/// CLI-shaped result of a successful Claude Code print run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct ClaudeCodeResult {
    /// `session_id` from the terminal record (feeds iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// Parsed `subtype`.
    pub(crate) stop_reason: ClaudeStopReason,
    /// Final assistant message (`result`).
    pub(crate) final_message: Option<String>,
    /// `num_turns`, when reported.
    pub(crate) num_turns: Option<u32>,
    /// `total_cost_usd`, when reported.
    pub(crate) total_cost_usd: Option<f64>,
}

/// CLI-shaped error hierarchy for Claude Code.
#[derive(Debug, Error)]
pub(crate) enum ClaudeCodeError {
    /// Context-window / token-limit detected in the output.
    #[error("claude hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A terminal `result` record with `is_error: true`.
    #[error("claude reported an error result (subtype `{subtype}`)")]
    Reported {
        /// The `subtype` of the failing record.
        subtype: String,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("claude was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal `result` record.
    #[error("claude produced no terminal result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Parse Claude Code's complete print-mode output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<ClaudeCodeResult, ClaudeCodeError> {
    let stdout = output.stdout_str();
    let terminal = cli_json::single_object(&stdout)
        .or_else(|| cli_json::last_event_of_type(&stdout, "result"));

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let Some(value) = terminal else {
        // Never produced a terminal record → never ran a turn.
        if let RawExit::Signal(sig) = output.exit {
            return Err(ClaudeCodeError::Signal(sig));
        }
        if let Some(detail) = detect_token_limit(&stdout) {
            return Err(ClaudeCodeError::TokenLimit(detail));
        }
        let stderr = output.stderr_str();
        if let Some(detail) = detect_token_limit(&stderr) {
            return Err(ClaudeCodeError::TokenLimit(detail));
        }
        return Err(ClaudeCodeError::NoResult { exit_code });
    };

    // Read each field independently from the (already-confirmed) JSON object.
    // `is_error` is the verdict, so it must not be hostage to a whole-struct
    // `from_value` that would fail — and silently default the verdict to
    // `false` = success — if any *unrelated* field (e.g. a float `num_turns`)
    // had an unexpected type.
    let is_error = value.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let subtype = value
        .get("subtype")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let final_message = value
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let num_turns = value
        .get("num_turns")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let total_cost_usd = value.get("total_cost_usd").and_then(Value::as_f64);

    if is_error {
        // A failing result: refine into token-limit when the text says so.
        if let Some(detail) = final_message
            .as_deref()
            .and_then(detect_token_limit)
            .or_else(|| detect_token_limit(&stdout))
        {
            return Err(ClaudeCodeError::TokenLimit(detail));
        }
        return Err(ClaudeCodeError::Reported {
            subtype: subtype.unwrap_or_default(),
            exit_code,
        });
    }

    Ok(ClaudeCodeResult {
        session_id,
        stop_reason: ClaudeStopReason::parse(subtype.as_deref()),
        final_message,
        num_turns,
        total_cost_usd,
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
    fn parses_successful_result_with_session_and_cost() {
        let json = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"sess-1","num_turns":3,"total_cost_usd":0.01}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.stop_reason, ClaudeStopReason::Success);
        assert_eq!(res.final_message.as_deref(), Some("done"));
        assert_eq!(res.num_turns, Some(3));
        assert_eq!(res.total_cost_usd, Some(0.01));
    }

    #[test]
    fn error_verdict_survives_a_mistyped_unrelated_field() {
        // `num_turns` is the wrong JSON type here. A whole-struct deserialize
        // would fail and default `is_error` to `false` (success). Reading
        // `is_error` independently keeps the failure verdict intact.
        let json = r#"{"type":"result","is_error":true,"num_turns":"oops"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClaudeCodeError::Reported { .. }), "got {err:?}");
    }

    #[test]
    fn error_result_maps_to_reported() {
        let json = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            ClaudeCodeError::Reported { ref subtype, exit_code: Some(1) } if subtype == "error_during_execution"
        ));
    }

    #[test]
    fn token_limit_in_error_result_is_detected() {
        let json = r#"{"type":"result","is_error":true,"result":"Error: context window exceeded"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClaudeCodeError::TokenLimit(_)));
    }

    #[test]
    fn no_terminal_result_on_nonzero_exit() {
        let err = interpret(&output("garbage\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClaudeCodeError::NoResult { exit_code: Some(1) }));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, ClaudeCodeError::Signal(9)));
    }

    #[test]
    fn token_limit_without_result_object_is_detected() {
        let err = interpret(&output("fatal: too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, ClaudeCodeError::TokenLimit(_)));
    }
}
