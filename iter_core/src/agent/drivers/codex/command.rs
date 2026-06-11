//! `CodexCommand` — the **Command level** for `OpenAI` Codex's `exec` mode.
//!
//! Owns the `codex exec --json` argv and parses the CLI's complete output
//! (a JSON-lines / JSONL event stream) into a CLI-shaped [`CodexResult`] or a
//! CLI-shaped [`CodexError`] hierarchy. Nothing here is iter-domain —
//! projecting these onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (Codex `exec --json`, 2026-05-29 ground truth)
//!
//! `codex exec --json` streams newline-delimited JSON events to stdout. Codex
//! wraps most events as `{"type": "...", ...}` (some builds nest the payload
//! under a `"msg"` object — both shapes are tolerated here). The events this
//! Command keys off of:
//!
//! * a **session-configured** event carrying a session/conversation id;
//! * a terminal **turn-status** record reporting `Completed` / `Failed` /
//!   `Interrupted`;
//! * **error** items carrying a `will_retry` flag;
//! * **token-usage** events.
//!
//! Exit-code surface: `0` turn ok · `1` startup/turn failure · `2` clap
//! bad-args · `130` SIGINT. A context-window overflow triggers *silent*
//! auto-compaction inside Codex, so the run may still exit `0`; a usage-limit
//! failure surfaces the message "You've hit your usage limit.".
//!
//! Field → conclusion chain: *did it run* = a terminal turn-status record is
//! present; *success/fail* = its status (`Completed` = success); *why* = the
//! error item + `will_retry`, refined to a token/usage limit when the stream
//! text says so. A process that never produced a turn-status record never ran
//! a turn: exit `2` is a bad-args launch failure, a signal is a signal error,
//! any other non-zero exit is a startup/"no result" failure.

use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Codex's usage-limit message — treated as a token/usage-limit class even
/// though it is not one of the generic context-window patterns.
const USAGE_LIMIT_MESSAGE: &str = "You've hit your usage limit.";

/// Clap argument-parse rejection exit code.
const EXIT_BAD_ARGS: i32 = 2;

/// Builds the Codex `exec --json` argv.
pub(crate) struct CodexCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, inserted between `exec`/`--json` and the
    /// positional prompt.
    pub(crate) args: &'a [String],
    /// The prompt, delivered as the final positional argument.
    pub(crate) prompt: &'a str,
}

impl CodexCommand<'_> {
    /// Build the `exec`-mode [`Command`]. Requests `--json` so the terminal
    /// turn-status record is machine-readable; the managed flags come first
    /// so the caller's `args` can still override / extend them, and the
    /// prompt is the final positional argument.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("exec");
        cmd.arg("--json");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd.arg(self.prompt);
        cmd
    }
}

/// Terminal turn status reported by Codex's turn-status record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexTurnStatus {
    /// Turn finished successfully (`Completed`).
    Completed,
    /// Turn ended in failure (`Failed`).
    Failed,
    /// Turn was interrupted (`Interrupted`).
    Interrupted,
    /// Any other status string the CLI emits.
    Other(String),
}

impl CodexTurnStatus {
    fn parse(status: &str) -> Self {
        match status.to_ascii_lowercase().as_str() {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => Self::Other(status.to_owned()),
        }
    }
}

/// CLI-shaped result of a successful Codex `exec` run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct CodexResult {
    /// Session / conversation id (feeds iter's session Factors), when the
    /// stream surfaced one.
    pub(crate) session_id: Option<String>,
    /// Parsed terminal turn status.
    pub(crate) turn_status: CodexTurnStatus,
    /// Final assistant message text, when the stream surfaced one.
    pub(crate) final_message: Option<String>,
    /// Total tokens used, when a token-usage event reported one.
    pub(crate) total_tokens: Option<u64>,
}

/// CLI-shaped error hierarchy for Codex `exec`.
#[derive(Debug, Error)]
pub(crate) enum CodexError {
    /// Context-window / usage-limit detected in the output.
    #[error("codex hit the usage/context limit: {0}")]
    TokenLimit(String),
    /// A terminal turn-status record reporting failure / interruption.
    #[error("codex reported turn status `{status}` (will_retry={will_retry})")]
    Reported {
        /// The status string of the failing record.
        status: String,
        /// `will_retry` flag from the accompanying error item, when present.
        will_retry: bool,
        /// Process exit code, when one accompanied the failure.
        exit_code: Option<i32>,
    },
    /// Bad CLI arguments (clap rejection, exit `2`). A misconfiguration, not
    /// a turn that ran.
    #[error("codex rejected the command-line arguments (exit code 2)")]
    BadArgs,
    /// The process was terminated by a signal before producing a turn status.
    #[error("codex was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a terminal turn-status
    /// record (startup failure / no result).
    #[error("codex produced no terminal turn status (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Does this object (or its nested `msg`) carry the given `type`/`kind`?
fn event_type_is(obj: &serde_json::Map<String, Value>, marker: &str) -> bool {
    let direct = obj
        .get("type")
        .or_else(|| obj.get("kind"))
        .and_then(Value::as_str)
        == Some(marker);
    let nested = obj
        .get("msg")
        .and_then(Value::as_object)
        .and_then(|m| m.get("type").or_else(|| m.get("kind")))
        .and_then(Value::as_str)
        == Some(marker);
    direct || nested
}

/// Return the event payload, unwrapping a nested `msg` object when present so
/// field lookups work for both the flat and the wrapped event shapes.
fn payload(value: &Value) -> &Value {
    value.get("msg").filter(|m| m.is_object()).unwrap_or(value)
}

/// Heuristic: does this object look like a terminal turn-status record? Codex
/// builds vary, so accept either an explicit `task_complete`/`turn_complete`
/// type or any object carrying a `status` field with a turn-status string.
fn is_turn_status(obj: &serde_json::Map<String, Value>) -> bool {
    if event_type_is(obj, "task_complete")
        || event_type_is(obj, "turn_complete")
        || event_type_is(obj, "turn.completed")
        || event_type_is(obj, "turn_status")
    {
        return true;
    }
    let status = obj
        .get("status")
        .or_else(|| {
            obj.get("msg")
                .and_then(Value::as_object)
                .and_then(|m| m.get("status"))
        })
        .and_then(Value::as_str);
    matches!(
        status.map(str::to_ascii_lowercase).as_deref(),
        Some("completed" | "failed" | "interrupted")
    )
}

/// First string value among `keys` on the event payload (flat or `msg`).
fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    let p = payload(value);
    keys.iter()
        .find_map(|k| p.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
}

/// Pull a turn-status string out of a terminal record.
fn turn_status_str(value: &Value) -> Option<String> {
    first_string(value, &["status", "turn_status"])
}

/// Pull a session/conversation id from any event that exposes one.
fn find_session_id(stdout: &str) -> Option<String> {
    let keys = &["session_id", "conversation_id", "thread_id"];
    cli_json::last_event_matching(stdout, |obj| {
        first_string(&Value::Object(obj.clone()), keys).is_some()
    })
    .and_then(|v| first_string(&v, keys))
}

/// Pull the final assistant message text from the latest agent-message event.
fn find_final_message(stdout: &str) -> Option<String> {
    cli_json::last_event_matching(stdout, |obj| {
        event_type_is(obj, "agent_message") || event_type_is(obj, "agent.message")
    })
    .and_then(|v| first_string(&v, &["message", "text", "last_agent_message"]))
}

/// Pull total token usage from the latest token-usage event.
fn find_total_tokens(stdout: &str) -> Option<u64> {
    cli_json::last_event_matching(stdout, |obj| {
        event_type_is(obj, "token_count") || event_type_is(obj, "token_usage")
    })
    .and_then(|v| {
        let p = payload(&v);
        p.get("total_tokens")
            .or_else(|| p.get("total_token_count"))
            .or_else(|| p.pointer("/usage/total_tokens"))
            .and_then(Value::as_u64)
    })
}

/// `will_retry` flag from the latest error item, when one is present.
fn find_will_retry(stdout: &str) -> bool {
    cli_json::last_event_matching(stdout, |obj| event_type_is(obj, "error")).is_some_and(|v| {
        payload(&v)
            .get("will_retry")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

/// Detect a token/usage-limit in `text`: the generic context-window patterns
/// plus Codex's literal usage-limit message.
fn detect_limit(text: &str) -> Option<String> {
    if let Some(detail) = detect_token_limit(text) {
        return Some(detail);
    }
    if text.contains(USAGE_LIMIT_MESSAGE) {
        return Some(USAGE_LIMIT_MESSAGE.to_owned());
    }
    None
}

/// Parse Codex's complete `exec --json` output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<CodexResult, CodexError> {
    let stdout = output.stdout_str();
    let terminal = cli_json::last_event_matching(&stdout, is_turn_status);

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let Some(value) = terminal else {
        // Never produced a terminal record → never ran a turn.
        if let RawExit::Signal(sig) = output.exit {
            return Err(CodexError::Signal(sig));
        }
        if let Some(detail) = detect_limit(&stdout).or_else(|| detect_limit(&output.stderr_str())) {
            return Err(CodexError::TokenLimit(detail));
        }
        if let RawExit::Code(EXIT_BAD_ARGS) = output.exit {
            return Err(CodexError::BadArgs);
        }
        return Err(CodexError::NoResult { exit_code });
    };

    let status_str = turn_status_str(&value).unwrap_or_default();
    let status = CodexTurnStatus::parse(&status_str);

    if !matches!(status, CodexTurnStatus::Completed) {
        // A failing / interrupted turn: refine into a usage/token limit when
        // the stream text says so before reporting the raw status.
        if let Some(detail) = detect_limit(&stdout) {
            return Err(CodexError::TokenLimit(detail));
        }
        return Err(CodexError::Reported {
            status: status_str,
            will_retry: find_will_retry(&stdout),
            exit_code,
        });
    }

    Ok(CodexResult {
        session_id: find_session_id(&stdout),
        turn_status: status,
        final_message: find_final_message(&stdout),
        total_tokens: find_total_tokens(&stdout),
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
    fn parses_completed_turn_with_session_and_usage() {
        let stream = concat!(
            "{\"type\":\"session_configured\",\"session_id\":\"sess-1\"}\n",
            "{\"type\":\"agent_message\",\"message\":\"all done\"}\n",
            "{\"type\":\"token_count\",\"total_tokens\":1234}\n",
            "{\"type\":\"task_complete\",\"status\":\"completed\"}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.turn_status, CodexTurnStatus::Completed);
        assert_eq!(res.final_message.as_deref(), Some("all done"));
        assert_eq!(res.total_tokens, Some(1234));
    }

    #[test]
    fn tolerates_msg_wrapped_event_shape() {
        let stream = concat!(
            "{\"msg\":{\"type\":\"session_configured\",\"session_id\":\"sess-2\"}}\n",
            "{\"msg\":{\"type\":\"task_complete\",\"status\":\"Completed\"}}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-2"));
        assert_eq!(res.turn_status, CodexTurnStatus::Completed);
    }

    #[test]
    fn failed_turn_maps_to_reported_with_will_retry() {
        let stream = concat!(
            "{\"type\":\"error\",\"message\":\"boom\",\"will_retry\":true}\n",
            "{\"type\":\"task_complete\",\"status\":\"failed\"}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            CodexError::Reported { ref status, will_retry: true, exit_code: Some(1) }
                if status == "failed"
        ));
    }

    #[test]
    fn usage_limit_message_maps_to_token_limit() {
        let stream = concat!(
            "{\"type\":\"error\",\"message\":\"You've hit your usage limit.\"}\n",
            "{\"type\":\"task_complete\",\"status\":\"failed\"}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CodexError::TokenLimit(_)));
    }

    #[test]
    fn context_window_without_terminal_record_is_token_limit() {
        let err = interpret(&output(
            "fatal: context window exceeded\n",
            RawExit::Code(1),
        ))
        .expect_err("err");
        assert!(matches!(err, CodexError::TokenLimit(_)));
    }

    #[test]
    fn bad_args_exit_maps_to_bad_args() {
        let err =
            interpret(&output("error: unexpected argument\n", RawExit::Code(2))).expect_err("err");
        assert!(matches!(err, CodexError::BadArgs));
    }

    #[test]
    fn no_terminal_record_on_nonzero_exit_is_no_result() {
        let err = interpret(&output("garbage\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CodexError::NoResult { exit_code: Some(1) }));
    }

    #[test]
    fn signal_without_record_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(2))).expect_err("err");
        assert!(matches!(err, CodexError::Signal(2)));
    }
}
