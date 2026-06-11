//! `CopilotCommand` — the **Command level** for GitHub Copilot CLI's print
//! mode.
//!
//! Owns the print-mode argv (`copilot -p … --allow-all-tools
//! --output-format json`) and parses the CLI's complete output into a
//! CLI-shaped [`CopilotResult`] or a CLI-shaped [`CopilotError`] hierarchy.
//! Nothing here is iter-domain — projecting these onto
//! [`AgentRun`](crate::agent::AgentRun) / [`AgentError`](crate::agent::AgentError)
//! is the driver's job (see the `From` impl in `mod.rs`).
//!
//! # Output contract (GitHub Copilot CLI 1.0.49, `--output-format json`)
//!
//! Exit-code surface is coarse: only `0` (the CLI ran) and `1` (everything
//! else). The verdict therefore lives in the JSON stream, not the exit code.
//! Two terminal record shapes matter:
//!
//! ```jsonc
//! // normal path
//! { "type": "result", "sessionId": "<id>", "exitCode": 0, "usage": { "premiumRequests": 1 } }
//! // failure path
//! { "type": "session.error", "errorType": "quota_exceeded", "errorCode": "...", "statusCode": 402 }
//! ```
//!
//! Field → conclusion chain: *did it run* = a terminal `result` record was
//! reached; *success/fail* = **presence of `session.error`** (its presence is
//! the failure signal); *why* = `errorType` + `statusCode`. Status mapping:
//! `402` quota / `429` rate are exhaustion classes; `401`/`403` auth; `5xx`
//! network; anything else other. Token/context-limit text in the stream is
//! also treated as exhaustion.

use serde::Deserialize;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// Builds the Copilot CLI print-mode argv.
pub(crate) struct CopilotCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
    /// The prompt, delivered as the value of `-p` (Copilot's print flag).
    pub(crate) prompt: &'a str,
}

impl CopilotCommand<'_> {
    /// Build the print-mode [`Command`]. Requests `--output-format json` so
    /// the terminal record is machine-readable and `--allow-all-tools` so the
    /// CLI does not block on per-tool confirmation (iter's sandbox is the real
    /// boundary). The managed flags come first so the caller's `args` can
    /// still override them.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("-p").arg(self.prompt);
        cmd.arg("--allow-all-tools");
        cmd.arg("--output-format").arg("json");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Usage figures reported in the terminal `result` record's `usage` object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CopilotUsage {
    /// `usage.premiumRequests`, when reported.
    pub(crate) premium_requests: Option<u64>,
}

/// CLI-shaped result of a successful Copilot print run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct CopilotResult {
    /// `sessionId` from the terminal record (feeds iter's session Factors).
    pub(crate) session_id: Option<String>,
    /// `exitCode` reported inside the terminal record, when present.
    pub(crate) exit_code: Option<i32>,
    /// Parsed `usage` figures.
    pub(crate) usage: CopilotUsage,
}

/// CLI-shaped error hierarchy for the Copilot CLI.
///
/// Carries the failure class plus the HTTP-ish `statusCode` the CLI surfaced.
/// No `Cancelled` / `Launch` variants live here — those are spawn-level
/// concerns owned by [`SpawnError`](crate::agent::process::SpawnError) and the
/// driver, not the output parser.
#[derive(Debug, Error)]
pub(crate) enum CopilotError {
    /// Quota exhausted (`session.error` with `statusCode` 402). Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`].
    ///
    /// [`AgentError::TokenLimit`]: crate::agent::AgentError::TokenLimit
    #[error("copilot quota exhausted (status {status:?}): {error_type}")]
    QuotaExhausted {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (expected 402).
        status: Option<u16>,
    },
    /// Rate limited (`session.error` with `statusCode` 429). Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`] (rate exhaustion is
    /// the closest domain class).
    ///
    /// [`AgentError::TokenLimit`]: crate::agent::AgentError::TokenLimit
    #[error("copilot rate limited (status {status:?}): {error_type}")]
    RateLimited {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (expected 429).
        status: Option<u16>,
    },
    /// Context-window / token-limit detected in the output text. Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`].
    ///
    /// [`AgentError::TokenLimit`]: crate::agent::AgentError::TokenLimit
    #[error("copilot hit the context/token limit: {0}")]
    TokenLimit(String),
    /// Authentication / authorization failure (`statusCode` 401/403).
    #[error("copilot authentication failed (status {status:?}): {error_type}")]
    Auth {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (401 or 403).
        status: Option<u16>,
    },
    /// Network / server-side failure (`statusCode` 5xx).
    #[error("copilot network error (status {status:?}): {error_type}")]
    Network {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (5xx).
        status: Option<u16>,
    },
    /// Any other reported `session.error` that does not fall into the classes
    /// above.
    #[error("copilot reported an error (`{error_type}`, status {status:?})")]
    Reported {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record, when present.
        status: Option<u16>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("copilot was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without a parseable terminal record or
    /// `session.error` (e.g. exit 1 with no JSON).
    #[error("copilot produced no terminal result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Raw `session.error` record, deserialized defensively. The CLI emits
/// camelCase keys (`errorType`, `statusCode`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionError {
    #[serde(default)]
    error_type: Option<String>,
    #[serde(default)]
    status_code: Option<u16>,
}

/// Raw terminal `result` record, deserialized defensively. The CLI emits
/// camelCase keys (`sessionId`, `exitCode`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalResult {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    #[serde(default)]
    premium_requests: Option<u64>,
}

/// Match a JSON object's `type`/`kind` marker against `marker`.
fn is_type(obj: &serde_json::Map<String, serde_json::Value>, marker: &str) -> bool {
    obj.get("type")
        .or_else(|| obj.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some(marker)
}

/// Parse Copilot's complete print-mode output into a result or error.
///
/// The `session.error` record — when present — is authoritative: its presence
/// *is* the failure signal, regardless of any terminal `result` that may also
/// appear. Otherwise the terminal `result` record carries the success path.
pub(crate) fn interpret(output: &CommandOutput) -> Result<CopilotResult, CopilotError> {
    let stdout = output.stdout_str();

    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    // Presence of `session.error` is the failure signal. Look for it first as
    // a single-object stream, then as a line in a JSON-lines stream.
    let error_value = cli_json::single_object(&stdout)
        .filter(|v| {
            v.as_object()
                .is_some_and(|obj| is_type(obj, "session.error"))
        })
        .or_else(|| cli_json::last_event_matching(&stdout, |obj| is_type(obj, "session.error")));

    if let Some(value) = error_value {
        let record: SessionError = serde_json::from_value(value).unwrap_or(SessionError {
            error_type: None,
            status_code: None,
        });
        return Err(classify_session_error(record, &stdout));
    }

    // No `session.error`: look for the terminal `result` record.
    let terminal = cli_json::single_object(&stdout)
        .filter(|v| v.as_object().is_some_and(|obj| is_type(obj, "result")))
        .or_else(|| cli_json::last_event_of_type(&stdout, "result"));

    let Some(value) = terminal else {
        // Never produced a terminal record → never ran a turn.
        if let RawExit::Signal(sig) = output.exit {
            return Err(CopilotError::Signal(sig));
        }
        if let Some(detail) = detect_token_limit(&stdout) {
            return Err(CopilotError::TokenLimit(detail));
        }
        let stderr = output.stderr_str();
        if let Some(detail) = detect_token_limit(&stderr) {
            return Err(CopilotError::TokenLimit(detail));
        }
        return Err(CopilotError::NoResult { exit_code });
    };

    let record: TerminalResult = serde_json::from_value(value).unwrap_or(TerminalResult {
        session_id: None,
        exit_code: None,
        usage: None,
    });

    Ok(CopilotResult {
        session_id: record.session_id,
        exit_code: record.exit_code,
        usage: CopilotUsage {
            premium_requests: record.usage.and_then(|u| u.premium_requests),
        },
    })
}

/// Map a `session.error` record onto the matching [`CopilotError`] class.
fn classify_session_error(record: SessionError, stdout: &str) -> CopilotError {
    let error_type = record.error_type.unwrap_or_default();
    let status = record.status_code;
    match status {
        Some(402) => CopilotError::QuotaExhausted { error_type, status },
        Some(429) => CopilotError::RateLimited { error_type, status },
        Some(401 | 403) => CopilotError::Auth { error_type, status },
        Some(code) if (500..600).contains(&code) => CopilotError::Network { error_type, status },
        _ => {
            // No exhaustion status, but the text may still describe a
            // context/token limit — refine into TokenLimit when it does.
            if let Some(detail) =
                detect_token_limit(&error_type).or_else(|| detect_token_limit(stdout))
            {
                return CopilotError::TokenLimit(detail);
            }
            CopilotError::Reported { error_type, status }
        }
    }
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
    fn parses_successful_result_with_session_and_usage() {
        let json =
            r#"{"type":"result","sessionId":"sess-1","exitCode":0,"usage":{"premiumRequests":2}}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("sess-1"));
        assert_eq!(res.exit_code, Some(0));
        assert_eq!(res.usage.premium_requests, Some(2));
    }

    #[test]
    fn parses_result_from_jsonl_stream() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"result\",\"sessionId\":\"s9\",\"exitCode\":0}\n",
        );
        let res = interpret(&output(stream, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("s9"));
    }

    #[test]
    fn quota_error_maps_to_quota_exhausted() {
        let json = r#"{"type":"session.error","errorType":"quota_exceeded","statusCode":402}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            CopilotError::QuotaExhausted {
                status: Some(402),
                ..
            }
        ));
    }

    #[test]
    fn rate_error_maps_to_rate_limited() {
        let json = r#"{"type":"session.error","errorType":"rate_limited","statusCode":429}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            CopilotError::RateLimited {
                status: Some(429),
                ..
            }
        ));
    }

    #[test]
    fn auth_error_maps_to_auth() {
        for code in [401u16, 403] {
            let json =
                format!(r#"{{"type":"session.error","errorType":"auth","statusCode":{code}}}"#);
            let err = interpret(&output(&json, RawExit::Code(1))).expect_err("err");
            assert!(
                matches!(err, CopilotError::Auth { .. }),
                "code {code}: {err:?}"
            );
        }
    }

    #[test]
    fn server_error_maps_to_network() {
        let json = r#"{"type":"session.error","errorType":"upstream","statusCode":503}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            CopilotError::Network {
                status: Some(503),
                ..
            }
        ));
    }

    #[test]
    fn unknown_status_error_maps_to_reported() {
        let json = r#"{"type":"session.error","errorType":"weird","statusCode":418}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            CopilotError::Reported { status: Some(418), ref error_type } if error_type == "weird"
        ));
    }

    #[test]
    fn session_error_with_token_limit_text_refines_to_token_limit() {
        let json = r#"{"type":"session.error","errorType":"context window exceeded for model"}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CopilotError::TokenLimit(_)));
    }

    #[test]
    fn session_error_is_authoritative_over_terminal_result() {
        let stream = concat!(
            "{\"type\":\"result\",\"sessionId\":\"s\",\"exitCode\":0}\n",
            "{\"type\":\"session.error\",\"errorType\":\"quota\",\"statusCode\":402}\n",
        );
        let err = interpret(&output(stream, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CopilotError::QuotaExhausted { .. }));
    }

    #[test]
    fn no_terminal_result_on_nonzero_exit() {
        let err = interpret(&output("garbage\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CopilotError::NoResult { exit_code: Some(1) }));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, CopilotError::Signal(9)));
    }

    #[test]
    fn token_limit_without_result_object_is_detected() {
        let err =
            interpret(&output("fatal: too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, CopilotError::TokenLimit(_)));
    }
}
