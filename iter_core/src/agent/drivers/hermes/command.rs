//! `HermesCommand` — the **Command level** for Nous Research Hermes' scripted
//! (`-z`) mode.
//!
//! Owns the `-z` rendered and parses the CLI's complete output + exit into a
//! CLI-shaped [`HermesResult`] or a CLI-shaped [`HermesError`] hierarchy.
//! Nothing here is iter-domain — projecting these onto
//! [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (Nous Hermes v0.14.0, `-z` scripted mode)
//!
//! **There is no JSON / machine-readable mode in `-z`.** Unlike Claude
//! (`--output-format json`) or Gemini (`-o json`), `hermes -z` emits the
//! final assistant text to stdout and nothing structured. stderr is
//! redirected to `/dev/null` for the duration of the run, and genuine
//! errors are appended to `~/.hermes/errors.log` — neither of which iter
//! captures here. The *only* in-process signal available is the exit code
//! plus a scan of whatever text reached stdout/stderr.
//!
//! Exit-code surface:
//!
//! * `0` — a response was produced. This is **unconditional**: it includes
//!   empty output and *most* provider/model failures, which Hermes
//!   stringifies into the response text rather than failing the process.
//!   Exit `0` therefore does **not** imply task success — it is merely the
//!   only "the agent ran" signal the scripted mode exposes.
//! * `1` — an uncaught Python exception: a launch / auth / config failure.
//!   A traceback is written to stderr. The agent never ran a turn.
//! * `2` — argparse / one-shot validation rejection (bad flags / args).
//!   The agent never ran a turn.
//! * `137` / `143` — terminated by `SIGKILL` / `SIGTERM` (surfaced as
//!   [`RawExit::Signal`]).
//!
//! Field → conclusion chain: *did it run* = exit `0` (the only positive
//! signal); *why it didn't* = exit `1` (traceback) vs `2` (bad args) vs a
//! signal. This is the **weakest** classifier of any driver: with no
//! structured output and stderr suppressed mid-run, a provider failure
//! baked into an exit-`0` response is indistinguishable from success.
//! Richer classification would require driving `hermes acp` / `hermes mcp`
//! or reading `~/.hermes/errors.log` — both out of scope for this driver.

use std::path::Path;

use thiserror::Error;
use tokio::process::Command;

use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};

/// argparse / one-shot validation rejection — bad flags or arguments.
const EXIT_BAD_ARGS: i32 = 2;
/// Uncaught Python exception — a launch / auth / config failure.
const EXIT_UNCAUGHT: i32 = 1;

/// Builds the Hermes scripted-mode (`-z`) rendered.
pub(crate) struct HermesCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// The prompt, delivered as the value of `-z`.
    pub(crate) prompt: &'a str,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
}

impl HermesCommand<'_> {
    /// Build the scripted-mode [`Command`]. `-z` suppresses banners,
    /// spinners, and cosmetic output and carries the prompt as its value;
    /// the managed flag comes first so the caller's `args` can still follow
    /// (and override) it.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("-z").arg(self.prompt);
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// CLI-shaped result of a successful Hermes scripted run.
///
/// Scripted mode exposes no session / conversation id (Hermes addresses
/// sessions only via its own `SQLite` store and `--resume`), so
/// [`HermesResult::session_id`] is always `None`. The final stdout text is
/// retained for completeness even though iter's domain result discards it.
#[derive(Debug, Clone, Default)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
#[allow(dead_code)]
pub(crate) struct HermesResult {
    /// Always `None`: `-z` mode surfaces no machine-readable session id.
    pub(crate) session_id: Option<String>,
    /// Final assistant text written to stdout, when non-empty.
    pub(crate) final_text: Option<String>,
}

/// CLI-shaped error hierarchy for Hermes' scripted mode.
///
/// There is no `Cancelled` or `Launch`-spawn variant here — those are owned
/// by the shared spawn primitive ([`SpawnError`](crate::agent::process)) and
/// collapsed before `interpret` runs.
#[derive(Debug, Error)]
pub(crate) enum HermesError {
    /// Context-window / token-limit detected in stdout or stderr text.
    #[error("hermes hit the context/token limit: {0}")]
    TokenLimit(String),
    /// Exit `1` — an uncaught Python exception (launch / auth / config
    /// failure). The agent never ran a turn. Carries a stderr/stdout snippet.
    #[error("hermes raised an uncaught exception: {0}")]
    Uncaught(String),
    /// Exit `2` — argparse / one-shot validation rejection. The agent never
    /// ran a turn.
    #[error("hermes rejected the invocation (bad arguments)")]
    BadArgs,
    /// An abnormal process exit that is neither a clean run nor a
    /// launch/config failure: any non-zero code other than `1`/`2`, or an
    /// indeterminate status (`exit_code = None`). Unlike exit `1`, this does
    /// not justify claiming the agent never ran a turn, so it is a generic
    /// ran-but-failed carrying the code when one exists — not a launch
    /// failure. Mirrors `AntigravityError::Failed` and the shared
    /// [`RawExit::into_failure`](crate::agent::process::RawExit::into_failure).
    #[error("hermes exited abnormally{}: {detail}", match .exit_code { Some(c) => format!(" with code {c}"), None => " (indeterminate status)".to_owned() })]
    Failed {
        /// The process exit code, or `None` for an indeterminate status.
        exit_code: Option<i32>,
        /// A stderr/stdout snippet, or a placeholder when none was captured.
        detail: String,
    },
    /// The process was terminated by a signal.
    #[error("hermes was terminated by signal {0}")]
    Signal(i32),
}

/// Trim a captured stream into a short, human-readable diagnostic snippet.
fn snippet(text: &str) -> String {
    const MAX: usize = 400;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_owned();
    }
    let cut: String = trimmed.chars().take(MAX).collect();
    format!("{cut}…")
}

/// Parse Hermes' complete scripted-mode output into a result or error.
///
/// Classification is exit-code-first, refined by a text scan for token
/// limits (the one router-relevant class detectable from text). Exit `0`
/// is the sole positive signal and yields a result even when stdout is
/// empty.
pub(crate) fn interpret(output: &CommandOutput) -> Result<HermesResult, HermesError> {
    let stdout = output.stdout_str();
    let stderr = output.stderr_str();

    // A token-limit excerpt anywhere in the captured text outranks the exit
    // code: it is the only class the router branches on, and Hermes may bake
    // it into an otherwise exit-`0` response.
    if let Some(detail) = detect_token_limit(&stdout).or_else(|| detect_token_limit(&stderr)) {
        return Err(HermesError::TokenLimit(detail));
    }

    match output.exit {
        RawExit::Code(0) => {
            let final_text = {
                let trimmed = stdout.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            };
            Ok(HermesResult {
                session_id: None,
                final_text,
            })
        }
        RawExit::Code(EXIT_UNCAUGHT) => {
            // The traceback lands on stderr; fall back to stdout when stderr
            // was suppressed (the `/dev/null` case), then to a bare label.
            let detail = {
                let s = snippet(&stderr);
                if s.is_empty() { snippet(&stdout) } else { s }
            };
            let detail = if detail.is_empty() {
                "uncaught exception (no diagnostic captured)".to_owned()
            } else {
                detail
            };
            Err(HermesError::Uncaught(detail))
        }
        RawExit::Code(EXIT_BAD_ARGS) => Err(HermesError::BadArgs),
        RawExit::Code(code) => {
            // Any other non-zero code: the process exited abnormally and `-z`
            // gives no finer signal. We cannot claim it never ran a turn (that
            // is exit `1`'s meaning), so this is a generic ran-but-failed.
            let detail = {
                let s = snippet(&stderr);
                if s.is_empty() { snippet(&stdout) } else { s }
            };
            let detail = if detail.is_empty() {
                "no diagnostic captured".to_owned()
            } else {
                detail
            };
            Err(HermesError::Failed {
                exit_code: Some(code),
                detail,
            })
        }
        RawExit::Signal(sig) => Err(HermesError::Signal(sig)),
        RawExit::Unknown => Err(HermesError::Failed {
            exit_code: None,
            detail: "exited with an indeterminate status".to_owned(),
        }),
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

    fn output_err(stdout: &str, stderr: &str, exit: RawExit) -> CommandOutput {
        CommandOutput {
            exit,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn exit_zero_with_text_is_ok_with_final_text() {
        let res = interpret(&output("the answer\n", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id, None);
        assert_eq!(res.final_text.as_deref(), Some("the answer"));
    }

    #[test]
    fn exit_zero_with_empty_output_is_ok() {
        let res = interpret(&output("   \n", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id, None);
        assert_eq!(res.final_text, None);
    }

    #[test]
    fn exit_one_maps_to_uncaught_with_stderr_snippet() {
        let err = interpret(&output_err(
            "",
            "Traceback (most recent call last): boom",
            RawExit::Code(1),
        ))
        .expect_err("err");
        match err {
            HermesError::Uncaught(detail) => {
                assert!(detail.contains("Traceback"), "got {detail:?}");
            }
            other => panic!("expected Uncaught, got {other:?}"),
        }
    }

    #[test]
    fn exit_one_falls_back_to_stdout_when_stderr_empty() {
        let err = interpret(&output("printed error detail", RawExit::Code(1))).expect_err("err");
        match err {
            HermesError::Uncaught(detail) => {
                assert!(detail.contains("printed error"), "got {detail:?}");
            }
            other => panic!("expected Uncaught, got {other:?}"),
        }
    }

    #[test]
    fn exit_one_with_no_text_still_maps_to_uncaught() {
        let err = interpret(&output("", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, HermesError::Uncaught(_)));
    }

    #[test]
    fn exit_two_maps_to_bad_args() {
        let err = interpret(&output("usage: hermes ...", RawExit::Code(2))).expect_err("err");
        assert!(matches!(err, HermesError::BadArgs));
    }

    #[test]
    fn other_nonzero_exit_maps_to_failed() {
        let err = interpret(&output("", RawExit::Code(3))).expect_err("err");
        assert!(
            matches!(err, HermesError::Failed { exit_code: Some(3), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn other_nonzero_exit_carries_stderr_detail() {
        let err = interpret(&output_err("", "segfault in tool", RawExit::Code(139)))
            .expect_err("err");
        match err {
            HermesError::Failed { exit_code, detail } => {
                assert_eq!(exit_code, Some(139));
                assert!(detail.contains("segfault"), "got {detail:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn unknown_exit_maps_to_failed_with_no_code() {
        let err = interpret(&output("", RawExit::Unknown)).expect_err("err");
        assert!(
            matches!(err, HermesError::Failed { exit_code: None, .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn signal_maps_to_signal() {
        let err = interpret(&output("", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, HermesError::Signal(9)));
    }

    #[test]
    fn token_limit_in_stdout_outranks_exit_zero() {
        let err = interpret(&output(
            "Error: context window exceeded\n",
            RawExit::Code(0),
        ))
        .expect_err("err");
        assert!(matches!(err, HermesError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_in_stderr_is_detected() {
        let err = interpret(&output_err(
            "",
            "fatal: too many tokens\n",
            RawExit::Code(1),
        ))
        .expect_err("err");
        assert!(matches!(err, HermesError::TokenLimit(_)));
    }

    #[test]
    fn builds_dash_z_argv_with_prompt() {
        let args: Vec<String> = vec!["--yolo".into()];
        let cmd = HermesCommand {
            program: "hermes",
            prompt: "hi",
            args: &args,
        }
        .build(Path::new("."));
        let std_cmd = cmd.as_std();
        let rendered: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rendered, vec!["-z", "hi", "--yolo"]);
    }
}
