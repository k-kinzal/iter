//! [`AntigravityDriver`] — Google Antigravity CLI (`agy`) integration.
//!
//! Antigravity CLI is Google's successor to Gemini CLI, announced at
//! I/O 2026. The binary, flag surface, and hook protocol all differ
//! from the Gemini CLI driver (`drivers/gemini/`).
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles:
//!
//!   ```text
//!   agy [--conversation <id>] --print <prompt> [extra-args...]
//!   ```
//!
//!   The prompt is delivered inline as the value of `--print`; nothing is fed
//!   on stdin. The child's complete stdout/stderr is captured so
//!   [`interpret`](AntigravityDriver::interpret) can classify the run. There
//!   is **no JSON mode** — see the output contract below for the text-marker
//!   classification.
//!
//! * [`AgentMode::Interactive`] — launches `agy` as a live TUI:
//!
//!   ```text
//!   agy [--conversation <id>] <prompt> [extra-args...]
//!   ```
//!
//!   Hook integration is deferred until the Antigravity hook JSON schema
//!   stabilizes (see the `hook` submodule); interactive mode therefore
//!   installs no hook bundle, so the default no-op
//!   [`prepare`](crate::agent::AgentDriver::prepare) /
//!   [`cleanup`](crate::agent::AgentDriver::cleanup) suffice. The agent exits
//!   after the TUI session and iter captures the exit status only.
//!   Interactive mode inherits stdin/stdout/stderr from the parent process;
//!   in non-tty environments use [`AgentMode::Headless`].
//!
//! # Output contract (Antigravity CLI `agy` 1.0.x, headless `-p`)
//!
//! **There is no JSON mode.** `agy -p` emits plain text to stdout plus
//! human-readable markers to stderr. This makes [`interpret`] the *weakest*
//! classifier of the mode-driven drivers: it has no reliable structured
//! signal, so it classifies by the process exit disposition plus text-marker
//! scanning.
//!
//! ## Exit code is overloaded
//!
//! `agy`'s exit code carries little information:
//!
//! * `0` — a clean run, **but also** the disposition reported for an
//!   auth-required prompt (it prints a login URL and exits `0`), a
//!   client-side kill (SIGTERM is trapped and turned into `0`; SIGKILL
//!   races to `0` or `137`), and some non-TTY launch failures.
//! * `2` — argument parse rejection.
//! * `126` / `127` — launch failure (not executable / not found).
//!
//! Unlike Gemini CLI, `agy` does **not** inherit Gemini's `41`–`58` fatal
//! startup range.
//!
//! ## Classification
//!
//! [`interpret`] scans stdout+stderr text and the exit disposition, in
//! priority order:
//!
//! 1. stderr contains `Authentication required` → auth failure (the agent
//!    never ran a turn — it printed a login URL and quit) → [`AgentError::Launch`].
//! 2. stderr contains `bubbletea: error opening TTY` → the TUI could not
//!    attach to a terminal (never ran) → [`AgentError::Launch`].
//! 3. [`detect_token_limit`] matches stdout or stderr → [`AgentError::TokenLimit`].
//! 4. exit `2` / `126` / `127` → [`AgentError::Launch`].
//! 5. exit by signal → [`AgentError::TerminatedBySignal`].
//! 6. otherwise a non-zero code → [`AgentError::Failed`]; a clean exit
//!    (after the markers above are ruled out) → a run.
//!
//! ## Cancellation
//!
//! `agy` cancellation via exit code is unreliable — a client SIGTERM is
//! trapped and reported as exit `0`. iter does **not** try to infer
//! cancellation from the child exit here. iter's own cancel token is
//! authoritative: the shared agent cycle turns a cancel into
//! [`AgentError::Cancelled`] before this layer ever sees a child exit.
//!
//! # Session persistence
//!
//! Unlike Gemini CLI, Antigravity has built-in session persistence via
//! `--conversation <id>`. When [`AntigravityDriver::conversation_id`] is set,
//! iter passes `--conversation <id>` on every invocation so the agent resumes
//! the same session. When unset, each iteration starts a fresh conversation.
//! `agy` does not echo the conversation id back in a machine-readable form, so
//! [`AgentRun::session_id`] is always `None`.
//!
//! # Construction
//!
//! [`AntigravityDriver`] exposes no defaults. Every field is required because
//! the value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::Path;

use antigravity_cli::{Antigravity, RunCommand, RunMode, RunOptions};
use async_trait::async_trait;
use thiserror::Error;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawExit, RawOutput, apply_user_env, detect_token_limit};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

pub(crate) mod hook;

/// stderr marker emitted when `agy` requires authentication. The CLI prints
/// a login URL and exits `0`, so this marker is the only reliable signal
/// that the run never began.
const MARKER_AUTH: &str = "Authentication required";

/// stderr marker emitted by the bubbletea TUI runtime when it cannot open a
/// controlling terminal (non-TTY launch).
const MARKER_TTY: &str = "bubbletea: error opening TTY";

/// CLI-shaped error hierarchy for Antigravity, projected onto [`AgentError`]
/// by the [`From`] impl below.
///
/// Note the absences relative to the JSON drivers: there is no `Cancelled`
/// (iter's cancel token owns that — see the module docs) and no spawn-`Launch`
/// I/O variant (a spawn I/O failure surfaces from the shared spawn primitive,
/// not here).
#[derive(Debug, Error)]
enum AntigravityError {
    /// stderr reported `Authentication required` — the CLI printed a login
    /// URL and exited without running a turn.
    #[error("antigravity requires authentication (it printed a login URL)")]
    Auth,
    /// stderr reported `bubbletea: error opening TTY` — the TUI could not
    /// attach to a controlling terminal (non-TTY launch).
    #[error("antigravity could not open a TTY for its interactive runtime")]
    LaunchTty,
    /// Context-window / token-limit detected in stdout or stderr.
    #[error("antigravity hit the context/token limit: {0}")]
    TokenLimit(String),
    /// Argument-parse rejection or a failure to exec the binary
    /// (exit `2` / `126` / `127`). The agent never ran a turn.
    #[error("antigravity failed to launch (exit code {0})")]
    Launch(i32),
    /// The process was terminated by a signal.
    #[error("antigravity was terminated by signal {0}")]
    Signal(i32),
    /// The process exited non-zero with no recognised marker.
    #[error("antigravity exited with a failure (exit code {exit_code:?})")]
    Failed {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

impl From<AntigravityError> for AgentError {
    /// Adapter projection: collapse Antigravity's CLI-shaped error hierarchy
    /// onto iter's minimal domain error. Auth and TTY failures mean the agent
    /// never ran, so they become [`AgentError::Launch`]; only
    /// [`AntigravityError::TokenLimit`] is router-relevant and preserved.
    fn from(err: AntigravityError) -> Self {
        match err {
            AntigravityError::Auth => {
                Self::Launch("antigravity requires authentication".to_owned())
            }
            AntigravityError::LaunchTty => {
                Self::Launch("antigravity could not open a TTY".to_owned())
            }
            AntigravityError::TokenLimit(detail) => Self::TokenLimit(detail),
            AntigravityError::Launch(code) => {
                Self::Launch(format!("antigravity failed to launch (exit code {code})"))
            }
            AntigravityError::Signal(sig) => Self::TerminatedBySignal(sig),
            AntigravityError::Failed { exit_code } => Self::Failed {
                code: exit_code,
                message: "antigravity exited with a failure".to_owned(),
            },
        }
    }
}

/// Classify Antigravity's complete headless output into a run or an error.
///
/// See the module docs for the full contract. The order of checks matters:
/// the text markers are scanned first because `agy` overloads exit `0` to
/// mean auth-required and TTY-failure as well as clean success.
fn classify_headless(raw: &RawOutput<'_>) -> Result<AgentRun, AgentError> {
    let stdout = raw.stdout_str();
    let stderr = raw.stderr_str();

    // 1. Auth: a login URL on stderr, regardless of exit code (it exits 0).
    if stderr.contains(MARKER_AUTH) {
        return Err(AntigravityError::Auth.into());
    }

    // 2. TTY/launch: bubbletea could not open a terminal.
    if stderr.contains(MARKER_TTY) {
        return Err(AntigravityError::LaunchTty.into());
    }

    // 3. Token limit: scan both streams.
    if let Some(detail) = detect_token_limit(&stdout).or_else(|| detect_token_limit(&stderr)) {
        return Err(AntigravityError::TokenLimit(detail).into());
    }

    // 4/5/6. Fall back to the exit disposition.
    match raw.exit {
        RawExit::Code(0) => Ok(AgentRun::empty()),
        RawExit::Code(code @ (2 | 126 | 127)) => Err(AntigravityError::Launch(code).into()),
        RawExit::Code(code) => Err(AntigravityError::Failed {
            exit_code: Some(code),
        }
        .into()),
        RawExit::Signal(sig) => Err(AntigravityError::Signal(sig).into()),
        RawExit::Unknown => Err(AntigravityError::Failed { exit_code: None }.into()),
    }
}

/// Antigravity CLI driver configuration.
#[derive(Debug, Clone)]
pub struct AntigravityDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Headless vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags.
    pub args: Vec<String>,
    /// Optional conversation ID for session persistence.
    pub conversation_id: Option<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

#[async_trait]
impl AgentDriver for AntigravityDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted: `agy`'s consumption of `TRACEPARENT` /
        // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so iter does not make its
        // traces *look* correlated without confirming the agent participates.
        //
        // Argv construction is delegated to `antigravity_cli`; this driver
        // keeps the mode selection, env application, and the text-marker
        // classification in `interpret`. `--conversation` (when set) is
        // rendered before the prompt so it is parsed as a flag, and the
        // caller's extra args are appended last. Nothing is fed on stdin: the
        // prompt is embedded in argv in both modes.
        // The prompt operand now rides inside the `RunMode` variant: headless
        // uses `--print <prompt>`, interactive seeds the TUI positionally.
        let prompt_text = prompt.as_str().to_owned();
        let (mode, io) = match self.mode {
            AgentMode::Headless => (RunMode::Print(prompt_text), StdioMode::Piped),
            AgentMode::Interactive => {
                (RunMode::Interactive(Some(prompt_text)), StdioMode::Inherit)
            }
        };
        let run = RunCommand {
            mode,
            options: RunOptions {
                conversation: self.conversation_id.clone(),
                ..RunOptions::default()
            },
        };
        let mut process = Antigravity::new(&self.command)
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
        AgentKind::Antigravity
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{drive, drive_capturing, fake_binary_script};

    fn driver(command: impl Into<String>, mode: AgentMode) -> AntigravityDriver {
        AntigravityDriver {
            command: command.into(),
            mode,
            args: Vec::new(),
            conversation_id: None,
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
    fn headless_command_emits_print_flag_then_prompt() {
        // Argv is now built by `antigravity_cli`, which renders the canonical
        // long-form `--print <prompt>` (equivalent to the CLI's `-p` alias).
        let d = driver("agy", AgentMode::Headless);
        let prompt = Prompt::from("hello-agy");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            argv(&command),
            vec!["--print".to_owned(), "hello-agy".to_owned()]
        );
        assert_eq!(command.stdin, None, "`--print` embeds the prompt in argv");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_adds_conversation_flag_when_set() {
        // Options render before the prompt (Go's `flag` parser stops at the
        // first positional), so `--conversation` now precedes `--print`.
        let mut d = driver("agy", AgentMode::Headless);
        d.conversation_id = Some("test-session-42".into());
        let prompt = Prompt::from("go");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(
            args,
            vec![
                "--conversation".to_owned(),
                "test-session-42".to_owned(),
                "--print".to_owned(),
                "go".to_owned(),
            ],
        );
    }

    #[test]
    fn headless_command_omits_conversation_flag_when_absent() {
        let d = driver("agy", AgentMode::Headless);
        let prompt = Prompt::from("go");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(
            !args.iter().any(|a| a == "--conversation"),
            "unset conversation_id must not emit --conversation: {args:?}",
        );
    }

    #[test]
    fn headless_command_places_conversation_before_extra_args() {
        let mut d = driver("agy", AgentMode::Headless);
        d.conversation_id = Some("sess-1".into());
        d.args = vec!["--print-timeout".into(), "600".into()];
        let prompt = Prompt::from("go");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(
            args,
            vec![
                "--conversation".to_owned(),
                "sess-1".to_owned(),
                "--print".to_owned(),
                "go".to_owned(),
                "--print-timeout".to_owned(),
                "600".to_owned(),
            ],
        );
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("agy", AgentMode::Headless);
        d.env = vec![("AGY_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("AGY_TEST_ENV_VAR")
                && v == Some(std::ffi::OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_places_conversation_before_positional_prompt() {
        let mut d = driver("agy", AgentMode::Interactive);
        d.conversation_id = Some("sess-1".into());
        let prompt = Prompt::from("interactive-prompt");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(
            argv(&command),
            vec![
                "--conversation".to_owned(),
                "sess-1".to_owned(),
                "interactive-prompt".to_owned(),
            ],
        );
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    #[test]
    fn interactive_command_passes_prompt_as_first_positional() {
        let d = driver("agy", AgentMode::Interactive);
        let prompt = Prompt::from("interactive-prompt");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert_eq!(args, vec!["interactive-prompt".to_owned()]);
    }

    // ----- interpret(): inbound translation (headless classification) -------

    #[test]
    fn headless_clean_exit_is_a_run_with_no_session_id() {
        let d = driver("agy", AgentMode::Headless);
        let run = d
            .interpret(&synth_output(RawExit::Code(0), "final answer\n", ""))
            .expect("clean exit is a run");
        assert_eq!(run.session_id, None);
    }

    #[test]
    fn headless_empty_clean_exit_is_still_a_run() {
        let d = driver("agy", AgentMode::Headless);
        d.interpret(&synth_output(RawExit::Code(0), "", ""))
            .expect("empty clean exit is still a run");
    }

    #[test]
    fn headless_auth_marker_outranks_clean_exit() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(0),
                "Visit https://login.example to authenticate\n",
                "Authentication required\n",
            ))
            .expect_err("auth must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn headless_tty_marker_maps_to_launch() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(
                RawExit::Code(0),
                "",
                "bubbletea: error opening TTY: device not configured\n",
            ))
            .expect_err("tty failure must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn headless_token_limit_in_stdout_is_detected() {
        let d = driver("agy", AgentMode::Headless);
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
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "", "too many tokens\n"))
            .expect_err("token limit must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn headless_arg_parse_exit_maps_to_launch() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(2), "", "unknown flag\n"))
            .expect_err("bad args must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn headless_not_found_exit_maps_to_launch() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(127), "", ""))
            .expect_err("exec failure must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn headless_other_nonzero_exit_maps_to_failed() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "", ""))
            .expect_err("abnormal exit must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn headless_signal_maps_to_terminated_by_signal() {
        let d = driver("agy", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), "", ""))
            .expect_err("signal must fail");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}",
        );
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

    #[test]
    fn interactive_interpret_judges_by_exit_only() {
        let d = driver("agy", AgentMode::Interactive);
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

    /// Fake `agy` print binary: echoes each argv arg to *stderr* (so a capture
    /// sink can observe the argv) and prints a final line to stdout so
    /// `interpret` reads a clean run.
    const FAKE_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf 'final answer'"#;

    #[tokio::test]
    async fn headless_passes_print_and_prompt_through() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-agy");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        let print_pos = args.iter().position(|a| *a == "--print").expect("--print");
        let prompt_pos = args.iter().position(|a| *a == "hello-agy").expect("prompt");
        assert!(print_pos < prompt_pos, "got {args:?}");
    }

    #[tokio::test]
    async fn headless_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$AGY_TEST_ENV_VAR\"");
        let mut d = driver(bin.to_string_lossy(), AgentMode::Headless);
        d.env = vec![("AGY_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert_eq!(sink.stdout().await, "env-value");
    }

    #[tokio::test]
    async fn headless_conversation_id_flag_is_forwarded() {
        let (_guard, bin) = fake_binary_script(FAKE_OK);
        let mut d = driver(bin.to_string_lossy(), AgentMode::Headless);
        d.conversation_id = Some("test-session-42".into());
        let prompt = Prompt::from("go");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        let echoed = sink.stderr().await;
        assert!(echoed.contains("--conversation"), "got {echoed:?}");
        assert!(echoed.contains("test-session-42"), "got {echoed:?}");
    }

    #[tokio::test]
    async fn headless_auth_marker_maps_to_launch_end_to_end() {
        let (_guard, bin) = fake_binary_script("printf 'Authentication required\\n' 1>&2\nexit 0");
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("auth must error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn headless_token_limit_marker_maps_to_token_limit_end_to_end() {
        let (_guard, bin) =
            fake_binary_script("printf 'Error: context window exceeded\\n'\nexit 0");
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("token limit must error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn headless_nonzero_exit_maps_to_failed_end_to_end() {
        let (_guard, bin) = fake_binary_script("exit 1");
        let d = driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("x");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("nonzero exit must error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn interactive_passes_prompt_as_positional_end_to_end() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let argv_file = tmp.path().join("argv.txt");
        // Interactive mode inherits stdio, so a capture sink sees nothing;
        // the fake records argv to a file the test reads back instead.
        let script = format!("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > {argv_file:?}");
        let (_guard, bin) = fake_binary_script(&script);
        let d = driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("interactive-prompt");
        let run = drive(d, tmp.path(), &prompt).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let argv_content = std::fs::read_to_string(&argv_file).expect("read argv");
        assert!(
            argv_content.contains("interactive-prompt"),
            "got {argv_content:?}",
        );
    }

    #[tokio::test]
    async fn interactive_nonzero_exit_is_failed_end_to_end() {
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = tempfile::TempDir::new().expect("tmp");
        let d = driver(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let err = drive(d, tmp.path(), &prompt)
            .await
            .expect_err("nonzero exit must error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }
}
