//! [`HermesDriver`] — Nous Research Hermes Agent (`hermes`) integration.
//!
//! Hermes Agent is an open-source AI coding agent by Nous Research
//! (MIT license). It is Python-based with persistent memory, a rich
//! hook system, and multiple execution backends.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles:
//!
//!   ```text
//!   hermes -z <prompt> [extra-args...]
//!   ```
//!
//!   The `-z` flag is the scripted mode that suppresses banners, spinners,
//!   and cosmetic output. The prompt is delivered as the value of `-z`
//!   (inline argv, not stdin), so nothing is fed on stdin.
//!
//! * [`AgentMode::Interactive`] — launches `hermes` as a modal TUI:
//!
//!   ```text
//!   hermes --tui <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until a schema stabilizes (see the `hook`
//!   submodule); interactive mode installs no hook bundle, so the default
//!   no-op [`prepare`](crate::agent::AgentDriver::prepare) /
//!   [`cleanup`](crate::agent::AgentDriver::cleanup) suffice. Interactive
//!   mode inherits stdin/stdout/stderr from the parent process; in non-tty
//!   environments use [`AgentMode::Headless`].
//!
//! # Output contract (`-z` scripted mode)
//!
//! The argv surface is modeled against the [`hermes_cli`] pin
//! ([`hermes_cli::SUPPORTED_HERMES_VERSION`]); the exit-code disposition below
//! was empirically observed against Nous Hermes v0.14.0 and has not been
//! re-observed against a later build (doing so would require invoking the
//! agent). It is treated as version-stable behavior, not a re-verified claim.
//!
//! **There is no JSON / machine-readable mode in `-z`.** `hermes -z` emits the
//! final assistant text to stdout and nothing structured; stderr is
//! redirected to `/dev/null` for the duration of the run, and genuine errors
//! are appended to `~/.hermes/errors.log` — neither of which iter captures.
//! The *only* in-process signal available is the exit code plus a scan of
//! whatever text reached stdout/stderr. [`interpret`](HermesDriver::interpret)
//! is therefore the **weakest** classifier of any driver:
//!
//! * `0` — a response was produced. This is **unconditional**: it includes
//!   empty output and *most* provider/model failures, which Hermes
//!   stringifies into the response text rather than failing the process.
//!   Exit `0` therefore does **not** imply task success — it is merely the
//!   only "the agent ran" signal the scripted mode exposes.
//! * `1` — an uncaught Python exception (launch / auth / config failure). A
//!   traceback is written to stderr. The agent never ran a turn → projected
//!   onto [`AgentError::Launch`].
//! * `2` — argparse / one-shot validation rejection (bad flags / args). The
//!   agent never ran a turn → projected onto [`AgentError::Launch`].
//! * any other non-zero code, or an indeterminate status → a generic
//!   ran-but-failed ([`AgentError::Failed`]); a signal →
//!   [`AgentError::TerminatedBySignal`].
//!
//! A token-limit excerpt anywhere in the captured text outranks the exit
//! code: it is the only class the router branches on, and Hermes may bake it
//! into an otherwise exit-`0` response.
//!
//! # Tool approval
//!
//! Hermes prompts for approval before executing tools. In non-TTY
//! environments (iter's use case), `--yolo` must be passed via `args` to
//! bypass prompts. iter does not hardcode this — the operator decides via
//! `args`.
//!
//! # Session persistence
//!
//! Hermes stores sessions in its own `SQLite` database and addresses them by
//! ID string. Operators can pass `--resume <id>` via `args` to resume a
//! specific session. iter does not manage Hermes sessions directly, and `-z`
//! mode surfaces no machine-readable session id, so
//! [`AgentRun::session_id`] is always `None`.
//!
//! # Construction
//!
//! [`HermesDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use async_trait::async_trait;
use hermes_cli::{Hermes, RunCommand};
use thiserror::Error;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawExit, RawOutput, apply_user_env, detect_token_limit};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

pub(crate) mod hook;

/// argparse / one-shot validation rejection — bad flags or arguments.
const EXIT_BAD_ARGS: i32 = 2;
/// Uncaught Python exception — a launch / auth / config failure.
const EXIT_UNCAUGHT: i32 = 1;

/// CLI-shaped error hierarchy for Hermes' scripted mode, projected onto
/// [`AgentError`] by the [`From`] impl below.
///
/// There is no `Cancelled` or spawn-`Launch` variant here — those are owned
/// by the shared agent cycle and never reach classification.
#[derive(Debug, Error)]
enum HermesError {
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
    /// failure.
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

impl From<HermesError> for AgentError {
    /// Adapter projection: collapse Hermes' CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`HermesError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]. Exit `1`
    /// (uncaught) and exit `2` (bad args) both mean the agent never ran a
    /// turn, so they project onto [`AgentError::Launch`]; any other abnormal
    /// exit ([`HermesError::Failed`]) is a generic ran-but-failed and projects
    /// onto [`AgentError::Failed`] carrying the code when one exists; a signal
    /// becomes [`AgentError::TerminatedBySignal`].
    fn from(err: HermesError) -> Self {
        match err {
            HermesError::TokenLimit(detail) => Self::TokenLimit(detail),
            HermesError::Uncaught(detail) => Self::Launch(format!("hermes: {detail}")),
            HermesError::BadArgs => {
                Self::Launch("hermes rejected the invocation (bad arguments)".to_owned())
            }
            HermesError::Failed { exit_code, detail } => Self::Failed {
                code: exit_code,
                message: format!("hermes: {detail}"),
            },
            HermesError::Signal(sig) => Self::TerminatedBySignal(sig),
        }
    }
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

/// Classify a completed `-z` run's output + exit into iter's domain.
///
/// Classification is exit-code-first, refined by a text scan for token limits
/// (the one router-relevant class detectable from text). Exit `0` is the sole
/// positive signal and yields a run even when stdout is empty.
fn classify_headless(raw: &RawOutput<'_>) -> Result<AgentRun, AgentError> {
    let stdout = raw.stdout_str();
    let stderr = raw.stderr_str();

    // A token-limit excerpt anywhere in the captured text outranks the exit
    // code: it is the only class the router branches on, and Hermes may bake
    // it into an otherwise exit-`0` response.
    if let Some(detail) = detect_token_limit(&stdout).or_else(|| detect_token_limit(&stderr)) {
        return Err(HermesError::TokenLimit(detail).into());
    }

    match raw.exit {
        RawExit::Code(0) => Ok(AgentRun::empty()),
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
            Err(HermesError::Uncaught(detail).into())
        }
        RawExit::Code(EXIT_BAD_ARGS) => Err(HermesError::BadArgs.into()),
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
            }
            .into())
        }
        RawExit::Signal(sig) => Err(HermesError::Signal(sig).into()),
        RawExit::Unknown => Err(HermesError::Failed {
            exit_code: None,
            detail: "exited with an indeterminate status".to_owned(),
        }
        .into()),
    }
}

/// Hermes CLI driver configuration.
#[derive(Debug, Clone)]
pub struct HermesDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Headless vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl AgentDriver for HermesDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted: Hermes' consumption of `TRACEPARENT` /
        // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so iter does not make its
        // traces *look* correlated without confirming the agent participates.
        //
        // Argv construction is delegated to `hermes_cli`; this driver keeps the
        // mode selection, env application, and the text-marker classification in
        // `interpret`. Headless renders `-z <prompt>` (the scripted mode that
        // suppresses banners/spinners); interactive renders `--tui <prompt>`.
        // The prompt rides in argv in both modes, so nothing is fed on stdin,
        // and the caller's extra args are appended last so they can still follow
        // (and override) the managed flags.
        let (run, io) = match self.mode {
            AgentMode::Headless => (RunCommand::oneshot(prompt.as_str()), StdioMode::Piped),
            AgentMode::Interactive => {
                (RunCommand::tui_prompt(prompt.as_str()), StdioMode::Inherit)
            }
        };
        let mut process = Hermes::new(&self.command)
            .with_current_dir(path)
            .to_process(&run);
        process.args(&self.args);
        apply_user_env(&mut process, &self.env);
        Ok(AgentCommand {
            process,
            stdin: None,
            io,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        let raw = RawOutput::from(output);
        match self.mode {
            // Interactive mode has no machine-readable output: the only signal
            // is the child's exit. A clean exit is a run; anything else fails.
            AgentMode::Interactive => match raw.exit.into_failure() {
                None => Ok(AgentRun::empty()),
                Some(err) => Err(err),
            },
            AgentMode::Headless => classify_headless(&raw),
        }
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Hermes
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{drive, drive_capturing, fake_binary_script};
    use tempfile::TempDir;

    fn driver(command: impl Into<String>, mode: AgentMode) -> HermesDriver {
        HermesDriver {
            command: command.into(),
            mode,
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

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn headless_command_emits_dash_z_prompt_over_argv() {
        let d = driver("hermes", AgentMode::Headless);
        let prompt = Prompt::from("hello-hermes");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            argv(&command),
            vec!["-z".to_owned(), "hello-hermes".to_owned()]
        );
        assert_eq!(command.stdin, None, "`-z` embeds the prompt in argv");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_appends_extra_args_after_prompt() {
        let mut d = driver("hermes", AgentMode::Headless);
        d.args = vec!["--yolo".into(), "--max-turns".into(), "30".into()];
        let prompt = Prompt::from("go");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(
            args,
            vec![
                "-z".to_owned(),
                "go".to_owned(),
                "--yolo".to_owned(),
                "--max-turns".to_owned(),
                "30".to_owned(),
            ],
        );
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("hermes", AgentMode::Headless);
        d.env = vec![("HERMES_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("HERMES_TEST_ENV_VAR")
                && v == Some(std::ffi::OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_emits_tui_then_prompt_and_inherits_stdio() {
        let d = driver("hermes", AgentMode::Interactive);
        let prompt = Prompt::from("interactive-prompt");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            argv(&command),
            vec!["--tui".to_owned(), "interactive-prompt".to_owned()],
        );
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    // ----- interpret(): inbound translation (headless classification) -------

    #[test]
    fn headless_exit_zero_with_text_is_a_run() {
        let d = driver("hermes", AgentMode::Headless);
        let run = d
            .interpret(&synth_output(RawExit::Code(0), "the answer\n", ""))
            .expect("clean exit is a run");
        assert_eq!(run.session_id, None);
    }

    #[test]
    fn headless_exit_zero_with_empty_output_is_a_run() {
        let d = driver("hermes", AgentMode::Headless);
        d.interpret(&synth_output(RawExit::Code(0), "   \n", ""))
            .expect("empty clean exit is still a run");
    }

    #[test]
    fn headless_exit_one_maps_to_launch_with_stderr_snippet() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                "",
                "Traceback (most recent call last): boom",
            ))
            .expect_err("uncaught exception must fail");
        let AgentError::Launch(msg) = err else {
            panic!("expected Launch, got {err:?}");
        };
        assert!(msg.contains("Traceback"), "got {msg:?}");
    }

    #[test]
    fn headless_exit_one_falls_back_to_stdout_when_stderr_empty() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "printed error detail", ""))
            .expect_err("uncaught exception must fail");
        let AgentError::Launch(msg) = err else {
            panic!("expected Launch, got {err:?}");
        };
        assert!(msg.contains("printed error"), "got {msg:?}");
    }

    #[test]
    fn headless_exit_one_with_no_text_still_maps_to_launch() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "", ""))
            .expect_err("uncaught exception must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn headless_exit_two_maps_to_launch_bad_args() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(2), "usage: hermes ...", ""))
            .expect_err("bad args must fail");
        let AgentError::Launch(msg) = err else {
            panic!("expected Launch, got {err:?}");
        };
        assert!(msg.contains("bad arguments"), "got {msg:?}");
    }

    #[test]
    fn headless_other_nonzero_exit_maps_to_failed() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(3), "", ""))
            .expect_err("abnormal exit must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(3), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn headless_other_nonzero_exit_carries_stderr_detail() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(139), "", "segfault in tool"))
            .expect_err("abnormal exit must fail");
        let AgentError::Failed { code, message } = err else {
            panic!("expected Failed, got {err:?}");
        };
        assert_eq!(code, Some(139));
        assert!(message.contains("segfault"), "got {message:?}");
    }

    #[test]
    fn headless_unknown_exit_maps_to_failed_with_no_code() {
        // `RawExit::Unknown` cannot round-trip through `ExitStatus`, so the
        // classifier is exercised directly on a synthesised `RawOutput`.
        let err = classify_headless(&RawOutput {
            exit: RawExit::Unknown,
            stdout: b"",
            stderr: b"",
        })
        .expect_err("indeterminate status must fail");
        assert!(
            matches!(err, AgentError::Failed { code: None, .. }),
            "got {err:?}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn headless_signal_maps_to_terminated_by_signal() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), "", ""))
            .expect_err("signal must fail");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}",
        );
    }

    #[test]
    fn headless_token_limit_in_stdout_outranks_exit_zero() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(0),
                "Error: context window exceeded\n",
                "",
            ))
            .expect_err("token limit must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn headless_token_limit_in_stderr_is_detected() {
        let d = driver("hermes", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                "",
                "fatal: too many tokens\n",
            ))
            .expect_err("token limit must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interactive_interpret_judges_by_exit_only() {
        let d = driver("hermes", AgentMode::Interactive);
        assert!(d.interpret(&synth_output(RawExit::Code(0), "", "")).is_ok());
        let err = d
            .interpret(&synth_output(RawExit::Code(7), "", ""))
            .expect_err("non-zero exit");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `hermes -z` binary: echoes each argv arg (one per line) to
    /// *stderr* so a capture sink can observe them, then prints a final
    /// response line to stdout so `interpret` reads a clean run.
    const FAKE_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf 'final response\n'"#;

    #[tokio::test]
    async fn headless_passes_dash_z_and_prompt_through() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-hermes");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert_eq!(args.first(), Some(&"-z"), "got {args:?}");
        assert_eq!(args.get(1), Some(&"hello-hermes"), "got {args:?}");
    }

    #[tokio::test]
    async fn headless_env_is_forwarded_to_child() {
        let (_guard, bin) =
            fake_binary_script("printf 'ENV=%s\\n' \"$HERMES_TEST_ENV_VAR\" 1>&2\nprintf 'ok\\n'");
        let mut d = driver(bin.to_string_lossy(), AgentMode::Headless);
        d.env = vec![("HERMES_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn headless_nonzero_exit_maps_to_launch_end_to_end() {
        // Exit 1 with a traceback on stderr → uncaught → Launch.
        let (_guard, bin) = fake_binary_script("printf 'Traceback: boom\\n' 1>&2\nexit 1");
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("nonzero exit is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn headless_token_limit_is_detected_end_to_end() {
        let (_guard, bin) = fake_binary_script("printf 'Error: context window exceeded\\n'");
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn interactive_nonzero_exit_maps_to_failed_end_to_end() {
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let d = driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let err = drive(d, tmp.path(), &prompt)
            .await
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }
}
