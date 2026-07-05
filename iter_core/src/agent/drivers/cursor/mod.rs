//! [`CursorDriver`] — Cursor `cursor-agent` CLI integration.
//!
//! Cursor's CLI is process-restart based: it has no hook installation and runs
//! to completion on each invocation. This driver is therefore **print-only**
//! — there is no interactive/TUI mode distinction.
//!
//! # Assumed CLI shape
//!
//! ```text
//! cursor-agent --print --output-format json [args...]
//! ```
//!
//! with the prompt written to stdin. `--print` causes the binary to emit a
//! single response and exit; `--output-format json` makes the terminal
//! `result` record machine-readable so the driver can recover the session id.
//!
//! The per-CLI argv construction and output parsing — including the subtle
//! success contract (presence of a terminal `result` record, *not* the
//! hard-coded `is_error` field) — live in the standalone [`cursor_cli`] crate.
//! This module is the Adapter: [`command`](CursorDriver::command) assembles the
//! [`AgentCommand`] from a [`PrintCommand`] and
//! [`interpret`](CursorDriver::interpret) projects the crate's CLI-shaped
//! [`PrintOutput`] onto iter's domain [`AgentRun`] / [`AgentError`]. Deciding
//! what a failure *means* for iter (token-limit routing, below-min-version,
//! signal, no-result) is driver work and stays here in [`CursorOutputError`].
//!
//! # Construction
//!
//! [`CursorDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use async_trait::async_trait;
use cursor_cli::{Cursor, PrintCommand, PrintOutput};
use thiserror::Error;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawExit, RawOutput, apply_user_env, detect_token_limit};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// Exit code `2`: the installed `cursor-agent` is below the minimum supported
/// version and never ran a turn.
const EXIT_BELOW_MIN_VERSION: i32 = 2;

/// Number of trailing stderr bytes carried in a no-result failure. Bounded so
/// a runaway child cannot bloat the error message; large enough to capture a
/// typical single-line CLI diagnostic.
const STDERR_TAIL_LIMIT: usize = 2_000;

/// CLI-shaped classification of a Cursor print run that cannot become a
/// successful [`AgentRun`].
///
/// The [`cursor_cli`] crate reports the stream faithfully (the terminal
/// `result` record, its session id, usage); deciding what a missing terminal
/// record or a non-zero exit *means* for iter is the driver's job, so this
/// intermediate error type lives here rather than in the OSS crate. The
/// [`From`] impl below projects it onto iter's minimal domain error.
///
/// Deliberately omits `Cancelled` and `Launch` (I/O-level spawn failures):
/// those are owned by the shared spawn primitive / driver, not this projection.
#[derive(Debug, Error)]
enum CursorOutputError {
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

impl From<CursorOutputError> for AgentError {
    /// Adapter projection: collapse Cursor's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`CursorOutputError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`];
    /// [`CursorOutputError::BelowMinVersion`] is a startup failure that never
    /// ran a turn, so it maps to [`AgentError::Launch`]; the rest become the
    /// generic failure / signal variants.
    fn from(err: CursorOutputError) -> Self {
        match err {
            CursorOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            CursorOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
            CursorOutputError::BelowMinVersion => Self::Launch(
                "cursor-agent is below the minimum supported version (exit 2)".to_owned(),
            ),
            CursorOutputError::NoResult { exit_code, detail } => Self::Failed {
                code: exit_code,
                message: format!("cursor-agent produced no terminal result: {detail}"),
            },
        }
    }
}

/// Cursor `cursor-agent` CLI driver configuration.
#[derive(Debug, Clone)]
pub struct CursorDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Additional arguments appended after the built-in print flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl AgentDriver for CursorDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // `cursor-agent --print --output-format json` from the crate, then the
        // caller's extra args appended so they can still extend/override the
        // managed flags. The prompt is delivered over stdin (this CLI reads it
        // there in print mode), not as a positional.
        let mut process = Cursor::new(&self.command)
            .with_current_dir(path)
            .to_process(&PrintCommand::json());
        for arg in &self.args {
            process.arg(arg);
        }
        apply_user_env(&mut process, &self.env);
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted: cursor-agent's consumption of `TRACEPARENT` /
        // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so — like the other
        // print-only drivers — iter does not make its traces *look*
        // correlated without confirming the agent actually participates.
        Ok(AgentCommand {
            process,
            stdin: Some(prompt.as_str().to_owned()),
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // Parse the CLI's `--print --output-format json` output with the OSS
        // crate, then project its faithful verdict onto iter's domain here: a
        // terminal `result` record is a run; its absence maps — via
        // `classify_failure` and the `From<CursorOutputError>` above — to the
        // matching domain error.
        let raw = RawOutput::from(output);
        let stdout = raw.stdout_str();
        let parsed = PrintOutput::parse(&stdout);
        if parsed.succeeded() {
            return Ok(AgentRun {
                session_id: parsed.session_id(),
            });
        }
        Err(classify_failure(&raw, &parsed, &stdout).into())
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

/// Build the failure error for a run that produced no terminal `result`.
///
/// Refinement order: token-limit (in stdout or stderr) → signal → exit `2`
/// below-min-version → a generic no-result error carrying the exit code and a
/// short diagnostic (a `type:"error"` record's message, else the stderr tail).
fn classify_failure(
    raw: &RawOutput<'_>,
    parsed: &PrintOutput,
    stdout: &str,
) -> CursorOutputError {
    let stderr = raw.stderr_str();
    if let Some(detail) = detect_token_limit(stdout).or_else(|| detect_token_limit(&stderr)) {
        return CursorOutputError::TokenLimit(detail);
    }
    if let RawExit::Signal(sig) = raw.exit {
        return CursorOutputError::Signal(sig);
    }
    if matches!(raw.exit, RawExit::Code(EXIT_BELOW_MIN_VERSION)) {
        return CursorOutputError::BelowMinVersion;
    }
    CursorOutputError::NoResult {
        exit_code: raw.exit.exit_code(),
        detail: failure_detail(parsed, &stderr),
    }
}

/// Short diagnostic for a no-result failure: prefer a `type:"error"` record's
/// message (surfaced by the crate), otherwise fall back to the tail of stderr.
fn failure_detail(parsed: &PrintOutput, stderr: &str) -> String {
    if let Some(message) = parsed.error_message() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RawExit;
    use crate::agent::testutil::{drive_capturing, fake_binary_script};
    use tempfile::TempDir;

    fn driver(command: impl Into<String>) -> CursorDriver {
        CursorDriver {
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    fn argv(command: &AgentCommand) -> Vec<String> {
        command
            .process
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn synth_output(exit: RawExit, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: exit.into_exit_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x","request_id":"req-x"}"#;

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn command_emits_print_json_and_stdin_prompt() {
        let d = driver("cursor-agent");
        let prompt = Prompt::from("hello-cursor");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--print".to_owned()), "got {args:?}");
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        assert_eq!(command.stdin.as_deref(), Some("hello-cursor"));
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn extra_args_are_appended_after_print_flags() {
        let mut d = driver("cursor-agent");
        d.args = vec!["--model".into(), "sonnet".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(args.contains(&"--print".to_owned()), "got {args:?}");
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"sonnet".to_owned()), "got {args:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("cursor-agent");
        d.env = vec![("CURSOR_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("CURSOR_TEST_ENV_VAR")
                && v == Some(std::ffi::OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    // ----- interpret(): inbound projection onto the domain ------------------

    #[test]
    fn interpret_success_result_extracts_session_id() {
        let d = driver("cursor-agent");
        let run = d
            .interpret(&synth_output(RawExit::Code(0), RESULT_OK, ""))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_no_terminal_result_maps_to_failed() {
        let d = driver("cursor-agent");
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "", "boom"))
            .expect_err("nonzero without result is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_maps_to_token_limit() {
        let d = driver("cursor-agent");
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                "",
                "context window exceeded",
            ))
            .expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_below_min_version_maps_to_launch() {
        let d = driver("cursor-agent");
        let err = d
            .interpret(&synth_output(RawExit::Code(2), "", "needs upgrade"))
            .expect_err("exit 2 is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn interpret_no_result_prefers_error_record_message() {
        let d = driver("cursor-agent");
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                r#"{"type":"error","message":"auth required"}"#,
                "noise",
            ))
            .expect_err("error record is a failure");
        assert!(
            matches!(&err, AgentError::Failed { message, .. } if message.contains("auth required")),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `cursor-agent` print binary: echoes each argv arg and its stdin to
    /// *stderr* (so the capture sink can observe them), then prints a valid
    /// terminal `result` JSON object to stdout.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
cat 1>&2
printf '%s' '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-x","request_id":"req-x"}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_flag_and_stdin() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("hello-cursor");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        assert!(echoed.lines().any(|l| l == "--print"), "got {echoed:?}");
        assert!(echoed.contains("hello-cursor"), "got {echoed:?}");
    }
}
