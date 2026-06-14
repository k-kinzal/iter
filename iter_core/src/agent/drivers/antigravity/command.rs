//! `AntigravityCommand` — the **Command level** for Antigravity's print mode.
//!
//! Owns the print-mode argv (`agy -p <prompt>`) and classifies the CLI's
//! complete output into a CLI-shaped [`AntigravityRun`] or a CLI-shaped
//! [`AntigravityError`] hierarchy. Nothing here is iter-domain — projecting
//! these onto [`AgentRun`](crate::agent::AgentRun) /
//! [`AgentError`](crate::agent::AgentError) is the driver's job (see the
//! `From` impl in `mod.rs`).
//!
//! # Output contract (Antigravity CLI `agy` 1.0.x)
//!
//! **There is no JSON mode.** `agy -p` emits plain text to stdout plus
//! human-readable markers to stderr. This makes the Command the *weakest*
//! classifier in the agent stack: it has no reliable structured signal, so
//! it classifies by the process exit disposition plus text-marker scanning.
//! That weakness is expected and documented here.
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
//! 1. stderr contains `Authentication required` → [`AntigravityError::Auth`]
//!    (the agent never ran a turn — it printed a login URL and quit).
//! 2. stderr contains `bubbletea: error opening TTY` →
//!    [`AntigravityError::LaunchTty`] (the TUI could not attach to a
//!    terminal — never ran).
//! 3. [`detect_token_limit`] matches stdout or stderr →
//!    [`AntigravityError::TokenLimit`].
//! 4. exit `2` / `126` / `127` → [`AntigravityError::Launch`].
//! 5. exit by signal → [`AntigravityError::Signal`].
//! 6. otherwise a non-zero code → [`AntigravityError::Failed`]; a clean exit
//!    (after the markers above are ruled out) → `Ok(AntigravityRun)`.
//!
//! ## Cancellation
//!
//! `agy` cancellation via exit code is unreliable — a client SIGTERM is
//! trapped and reported as exit `0`. iter does **not** try to infer
//! cancellation from the child exit here. iter's own cancel token is
//! authoritative: [`spawn_capture`](crate::agent::process::spawn_capture)
//! returns `SpawnError::Cancelled` (→ `AgentError::Cancelled`) when iter
//! cancels, so this layer never sees a cancelled run as a child exit.

use thiserror::Error;
use tokio::process::Command;

use crate::agent::process::{CommandOutput, RawExit, detect_token_limit};
use std::path::Path;

/// stderr marker emitted when `agy` requires authentication. The CLI prints
/// a login URL and exits `0`, so this marker is the only reliable signal
/// that the run never began.
const MARKER_AUTH: &str = "Authentication required";

/// stderr marker emitted by the bubbletea TUI runtime when it cannot open a
/// controlling terminal (non-TTY launch).
const MARKER_TTY: &str = "bubbletea: error opening TTY";

/// Builds the Antigravity print-mode argv.
///
/// There is no format flag — `agy` has no JSON mode — so the managed flags
/// are just `-p <prompt>` followed by an optional `--conversation <id>` and
/// the caller's extra args.
pub(crate) struct AntigravityCommand<'a> {
    /// Binary name or path.
    pub(crate) program: &'a str,
    /// The prompt, delivered inline as the value of `-p`.
    pub(crate) prompt: &'a str,
    /// Optional conversation id for session persistence.
    pub(crate) conversation_id: Option<&'a str>,
    /// Caller-supplied extra args, appended after the managed flags.
    pub(crate) args: &'a [String],
}

impl AntigravityCommand<'_> {
    /// Build the print-mode [`Command`]. The prompt is embedded inline as the
    /// value of `-p`; `--conversation` (when set) precedes the caller's extra
    /// args so users can still append their own flags.
    pub(crate) fn build(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.current_dir(path);
        cmd.arg("-p").arg(self.prompt);
        if let Some(id) = self.conversation_id {
            cmd.arg("--conversation").arg(id);
        }
        for arg in self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

/// CLI-shaped result of a successful Antigravity print run.
///
/// `agy` exposes no machine-readable session id in print mode, so
/// [`session_id`](Self::session_id) is always `None`. The captured stdout is
/// surfaced as [`final_text`](Self::final_text) for completeness, since it is
/// the only artifact the CLI produces.
#[derive(Debug, Clone)]
// Captures the CLI's complete result per the no-output-loss design; iter
// currently consumes only `session_id`, so the rest is read by this
// module's tests and reserved for future Factors.
pub(crate) struct AntigravityRun {
    /// Always `None` — `agy` print mode reports no session id.
    pub(crate) session_id: Option<String>,
    /// The captured stdout, when non-empty. Informational only.
    pub(crate) final_text: Option<String>,
}

/// CLI-shaped error hierarchy for Antigravity.
///
/// Note the absences relative to the JSON drivers: there is no `Cancelled`
/// (iter's cancel token owns that — see the module docs) and no `Launch`
/// I/O variant (a spawn I/O failure surfaces as `SpawnError::Launch` from
/// the shared spawn primitive, not here).
#[derive(Debug, Error)]
pub(crate) enum AntigravityError {
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

/// Classify Antigravity's complete print-mode output into a result or error.
///
/// See the module docs for the full contract. The order of checks matters:
/// the text markers are scanned first because `agy` overloads exit `0` to
/// mean auth-required and TTY-failure as well as clean success.
pub(crate) fn interpret(output: &CommandOutput) -> Result<AntigravityRun, AntigravityError> {
    let stdout = output.stdout_str();
    let stderr = output.stderr_str();

    // 1. Auth: a login URL on stderr, regardless of exit code (it exits 0).
    if stderr.contains(MARKER_AUTH) {
        return Err(AntigravityError::Auth);
    }

    // 2. TTY/launch: bubbletea could not open a terminal.
    if stderr.contains(MARKER_TTY) {
        return Err(AntigravityError::LaunchTty);
    }

    // 3. Token limit: scan both streams.
    if let Some(detail) = detect_token_limit(&stdout).or_else(|| detect_token_limit(&stderr)) {
        return Err(AntigravityError::TokenLimit(detail));
    }

    // 4/5/6. Fall back to the exit disposition.
    match output.exit {
        RawExit::Code(0) => Ok(AntigravityRun {
            session_id: None,
            final_text: non_empty(stdout.trim()),
        }),
        RawExit::Code(code @ (2 | 126 | 127)) => Err(AntigravityError::Launch(code)),
        RawExit::Code(code) => Err(AntigravityError::Failed {
            exit_code: Some(code),
        }),
        RawExit::Signal(sig) => Err(AntigravityError::Signal(sig)),
        RawExit::Unknown => Err(AntigravityError::Failed { exit_code: None }),
    }
}

/// `Some(owned)` when `text` is non-empty, else `None`.
fn non_empty(text: &str) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
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
    fn build_emits_dash_p_then_prompt() {
        let cmd = AntigravityCommand {
            program: "agy",
            prompt: "hello",
            conversation_id: None,
            args: &[],
        }
        .build(Path::new("."));
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-p".to_owned(), "hello".to_owned()]);
    }

    #[test]
    fn build_places_conversation_before_extra_args() {
        let extra = vec!["--print-timeout".to_owned(), "600".to_owned()];
        let cmd = AntigravityCommand {
            program: "agy",
            prompt: "go",
            conversation_id: Some("sess-1"),
            args: &extra,
        }
        .build(Path::new("."));
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-p".to_owned(),
                "go".to_owned(),
                "--conversation".to_owned(),
                "sess-1".to_owned(),
                "--print-timeout".to_owned(),
                "600".to_owned(),
            ]
        );
    }

    #[test]
    fn clean_exit_is_ok_with_no_session_id() {
        let res = interpret(&output("final answer\n", "", RawExit::Code(0))).expect("ok");
        assert_eq!(res.session_id, None);
        assert_eq!(res.final_text.as_deref(), Some("final answer"));
    }

    #[test]
    fn empty_clean_exit_has_no_final_text() {
        let res = interpret(&output("", "", RawExit::Code(0))).expect("ok");
        assert_eq!(res.final_text, None);
    }

    #[test]
    fn auth_marker_wins_over_clean_exit() {
        let err = interpret(&output(
            "Visit https://login.example to authenticate\n",
            "Authentication required\n",
            RawExit::Code(0),
        ))
        .expect_err("err");
        assert!(matches!(err, AntigravityError::Auth));
    }

    #[test]
    fn tty_marker_maps_to_launch_tty() {
        let err = interpret(&output(
            "",
            "bubbletea: error opening TTY: device not configured\n",
            RawExit::Code(0),
        ))
        .expect_err("err");
        assert!(matches!(err, AntigravityError::LaunchTty));
    }

    #[test]
    fn token_limit_in_stdout_is_detected() {
        let err = interpret(&output(
            "Error: context window exceeded\n",
            "",
            RawExit::Code(0),
        ))
        .expect_err("err");
        assert!(matches!(err, AntigravityError::TokenLimit(_)));
    }

    #[test]
    fn token_limit_in_stderr_is_detected() {
        let err = interpret(&output("", "too many tokens\n", RawExit::Code(1))).expect_err("err");
        assert!(matches!(err, AntigravityError::TokenLimit(_)));
    }

    #[test]
    fn arg_parse_exit_maps_to_launch() {
        let err = interpret(&output("", "unknown flag\n", RawExit::Code(2))).expect_err("err");
        assert!(matches!(err, AntigravityError::Launch(2)));
    }

    #[test]
    fn not_found_exit_maps_to_launch() {
        let err = interpret(&output("", "", RawExit::Code(127))).expect_err("err");
        assert!(matches!(err, AntigravityError::Launch(127)));
    }

    #[test]
    fn other_nonzero_exit_maps_to_failed() {
        let err = interpret(&output("", "", RawExit::Code(1))).expect_err("err");
        assert!(matches!(
            err,
            AntigravityError::Failed { exit_code: Some(1) }
        ));
    }

    #[test]
    fn signal_exit_maps_to_signal() {
        let err = interpret(&output("", "", RawExit::Signal(9))).expect_err("err");
        assert!(matches!(err, AntigravityError::Signal(9)));
    }

    #[test]
    fn unknown_exit_maps_to_failed_without_code() {
        let err = interpret(&output("", "", RawExit::Unknown)).expect_err("err");
        assert!(matches!(err, AntigravityError::Failed { exit_code: None }));
    }
}
