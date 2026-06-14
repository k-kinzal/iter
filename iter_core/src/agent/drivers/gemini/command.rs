//! `GeminiCommand` — the **Command level** for the Gemini CLI's print mode.
//!
//! Owns the print-mode rendered (`gemini -p <prompt> -o json`) and parses the
//! CLI's complete output + exit into a CLI-shaped [`GeminiRun`] or a
//! CLI-shaped [`GeminiError`] hierarchy. Nothing here is iter-domain —
//! projecting these onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (Gemini CLI 0.41.2, `-o json`)
//!
//! The whole stream is a single JSON object — `{ "response": <text>,
//! "stats": { "tokens": {...} }, "error": { "type", "message", "code" } }`.
//! Field → conclusion chain: *did it run* = a JSON object was produced;
//! *success/fail* = **presence of an `error` field**; *why* = `error.type`
//! / `error.code` (plus the fatal startup exit codes below).
//!
//! Gemini does not expose a session / conversation id in `-o json` output,
//! so [`GeminiRun::session_id`] is parsed defensively and is `None` in
//! practice.
//!
//! # Exit-code surface
//!
//! `0` ran a turn · `1` runtime error · `41`/`42`/`44`/`52`/`53` fatal
//! startup failures (auth / input / sandbox / config / turn-limit) that
//! mean the agent never ran a turn. The startup codes map to
//! [`GeminiError::Startup`], which the driver projects onto
//! [`AgentError::Launch`](crate::agent::AgentError).

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tokio::process::Command;

use crate::agent::cli_json;
use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};

/// Fatal startup exit codes (auth / input / sandbox / config / turn-limit);
/// a process that exits with one never ran a turn → `AgentError::Launch`.
const STARTUP_EXIT_CODES: &[i32] = &[41, 42, 44, 52, 53];

/// Builds the Gemini print-mode rendered.
pub(crate) struct GeminiCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// The prompt, delivered as the value of `-p`.
    pub(crate) prompt: &'a str,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
}

impl GeminiCommand<'_> {
    /// Build the print-mode [`Command`]. Requests `-o json` so the terminal
    /// record is machine-readable; the managed flags come first so the
    /// caller's `args` can still override them.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("-p").arg(self.prompt);
        cmd.arg("-o").arg("json");
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// Token statistics reported in the terminal record's `stats.tokens`
/// (input / prompt, output / completion, and total counts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GeminiTokenStats {
    pub(crate) input: Option<u64>,
    pub(crate) output: Option<u64>,
    pub(crate) total: Option<u64>,
}

/// CLI-shaped result of a successful Gemini print run.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
pub(crate) struct GeminiRun {
    /// Session / conversation id, if Gemini ever exposes one (it does not in
    /// `-o json` today). Parsed defensively; feeds iter's session Factors.
    pub(crate) session_id: Option<String>,
    /// Final assistant message (`response`).
    pub(crate) response: Option<String>,
    /// Token statistics from `stats`, when reported.
    pub(crate) tokens: GeminiTokenStats,
}

/// CLI-shaped error hierarchy for the Gemini CLI.
#[derive(Debug, Error)]
pub(crate) enum GeminiError {
    /// Context-window / token-limit detected in the output or `error.message`.
    #[error("gemini hit the context/token limit: {0}")]
    TokenLimit(String),
    /// A fatal startup exit code (auth / input / sandbox / config /
    /// turn-limit). The agent never ran a turn.
    #[error("gemini failed to start (exit code {exit_code})")]
    Startup {
        /// The fatal startup exit code (one of `STARTUP_EXIT_CODES`).
        exit_code: i32,
        /// Diagnostic message, when one was parsed from the JSON `error`.
        message: Option<String>,
    },
    /// A terminal record carrying an `error` field (in-band failure).
    #[error("gemini reported an error result")]
    Reported {
        /// `error.type`, when present.
        error_type: Option<String>,
        /// `error.message`, when present.
        message: Option<String>,
        /// `error.code`, or the process exit code when no JSON code was given.
        code: Option<i32>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("gemini was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without ever producing a JSON result object.
    #[error("gemini produced no result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

/// Raw terminal record, deserialized from the `-o json` object.
#[derive(Debug, Default, Deserialize)]
struct TerminalRecord {
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    stats: Option<Stats>,
    #[serde(default)]
    error: Option<ErrorRecord>,
}

#[derive(Debug, Default, Deserialize)]
struct Stats {
    #[serde(default)]
    tokens: Option<TokenCounts>,
}

#[derive(Debug, Default, Deserialize)]
struct TokenCounts {
    #[serde(default)]
    input: Option<u64>,
    #[serde(default)]
    output: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorRecord {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<i32>,
}

/// Is this exit code one of the fatal startup codes?
fn is_startup_code(exit_code: Option<i32>) -> Option<i32> {
    exit_code.filter(|c| STARTUP_EXIT_CODES.contains(c))
}

/// Does this `error.type` look like a context/token-limit class?
fn is_context_error_type(error_type: Option<&str>) -> bool {
    error_type.is_some_and(|t| {
        let lower = t.to_ascii_lowercase();
        lower.contains("context") || lower.contains("token")
    })
}

/// Parse the Gemini CLI's complete print-mode output into a result or error.
pub(crate) fn interpret(output: &CommandOutput) -> Result<GeminiRun, GeminiError> {
    let stdout = output.stdout_str();
    let exit_code = match output.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    let record = cli_json::single_object(&stdout)
        .map(|value| serde_json::from_value::<TerminalRecord>(value).unwrap_or_default());

    let Some(record) = record else {
        // No JSON object → the agent never produced a result.
        if let RawExit::Signal(sig) = output.exit {
            return Err(GeminiError::Signal(sig));
        }
        if let Some(detail) =
            detect_token_limit(&stdout).or_else(|| detect_token_limit(&output.stderr_str()))
        {
            return Err(GeminiError::TokenLimit(detail));
        }
        if let Some(code) = is_startup_code(exit_code) {
            return Err(GeminiError::Startup {
                exit_code: code,
                message: None,
            });
        }
        return Err(GeminiError::NoResult { exit_code });
    };

    if let Some(err) = record.error {
        let message = err.message.clone();
        // Refine into token-limit when the type or message says so.
        if is_context_error_type(err.error_type.as_deref()) {
            let detail = message
                .as_deref()
                .and_then(detect_token_limit)
                .or_else(|| message.clone())
                .unwrap_or_else(|| "context/token limit".to_owned());
            return Err(GeminiError::TokenLimit(detail));
        }
        if let Some(detail) = message
            .as_deref()
            .and_then(detect_token_limit)
            .or_else(|| detect_token_limit(&stdout))
        {
            return Err(GeminiError::TokenLimit(detail));
        }
        if let Some(code) = is_startup_code(exit_code) {
            return Err(GeminiError::Startup {
                exit_code: code,
                message,
            });
        }
        return Err(GeminiError::Reported {
            error_type: err.error_type,
            message,
            code: err.code.or(exit_code),
        });
    }

    // No `error` field, but a startup exit code still overrides a stray object.
    if let Some(code) = is_startup_code(exit_code) {
        return Err(GeminiError::Startup {
            exit_code: code,
            message: None,
        });
    }

    let tokens = record
        .stats
        .and_then(|s| s.tokens)
        .map(|t| GeminiTokenStats {
            input: t.input,
            output: t.output,
            total: t.total,
        })
        .unwrap_or_default();

    Ok(GeminiRun {
        session_id: record.session_id,
        response: record.response,
        tokens,
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

    fn output_err(stderr: &str, exit: RawExit) -> CommandOutput {
        CommandOutput {
            exit,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_successful_response_and_tokens() {
        let json = r#"{"response":"done","stats":{"tokens":{"input":10,"output":20,"total":30}}}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.response.as_deref(), Some("done"));
        assert_eq!(res.session_id, None);
        assert_eq!(res.tokens.input, Some(10));
        assert_eq!(res.tokens.output, Some(20));
        assert_eq!(res.tokens.total, Some(30));
    }

    #[test]
    fn parses_session_id_when_present() {
        let json = r#"{"response":"ok","session_id":"conv-1"}"#;
        let res = interpret(&output(json, RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id.as_deref(), Some("conv-1"));
    }

    #[test]
    fn error_field_maps_to_reported() {
        let json = r#"{"error":{"type":"ApiError","message":"boom","code":7}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            GeminiError::Reported { code: Some(7), ref error_type, .. }
                if error_type.as_deref() == Some("ApiError")
        ));
    }

    #[test]
    fn error_field_falls_back_to_exit_code_when_no_json_code() {
        let json = r#"{"error":{"type":"ApiError","message":"boom"}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GeminiError::Reported { code: Some(1), .. }));
    }

    #[test]
    fn context_error_type_maps_to_token_limit() {
        let json = r#"{"error":{"type":"ContextLengthExceeded","message":"too big"}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GeminiError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_in_error_message_is_detected() {
        let json = r#"{"error":{"type":"Other","message":"context window exceeded"}}"#;
        let err = interpret(&output(json, RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GeminiError::TokenLimit(_)));
    }

    #[test]
    fn startup_exit_code_with_error_maps_to_startup() {
        let json = r#"{"error":{"type":"AuthError","message":"not logged in"}}"#;
        let err = interpret(&output(json, RawExit::Code(41))).expect_err("err");
        assert!(matches!(
            err,
            GeminiError::Startup { exit_code: 41, ref message }
                if message.as_deref() == Some("not logged in")
        ));
    }

    #[test]
    fn each_startup_code_without_json_maps_to_startup() {
        for code in STARTUP_EXIT_CODES {
            let err = interpret(&output("not json", RawExit::Code(*code))).expect_err("err");
            assert!(
                matches!(err, GeminiError::Startup { exit_code, .. } if exit_code == *code),
                "code {code} got {err:?}"
            );
        }
    }

    #[test]
    fn no_json_on_nonzero_exit_maps_to_no_result() {
        let err = interpret(&output("garbage\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GeminiError::NoResult { exit_code: Some(1) }));
    }

    #[test]
    fn signal_without_result_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, GeminiError::Signal(9)));
    }

    #[test]
    fn token_limit_in_stderr_without_json_is_detected() {
        let err =
            interpret(&output_err("fatal: too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, GeminiError::TokenLimit(_)));
    }

    #[test]
    fn builds_print_argv_with_prompt_and_json_format() {
        let args: Vec<String> = vec![];
        let cmd = GeminiCommand {
            program: "gemini",
            prompt: "hi",
            args: &args,
        }
        .build(Path::new("."));
        let std_cmd = cmd.as_std();
        let rendered: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rendered, vec!["-p", "hi", "-o", "json"]);
    }
}
