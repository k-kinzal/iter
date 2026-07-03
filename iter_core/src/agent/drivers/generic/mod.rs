//! [`GenericDriver`] — run any configured CLI command as an agent.
//!
//! The escape hatch when none of the first-class integrations fit: any
//! command-line tool that consumes a prompt and writes back to stdout can be
//! driven through it. There is no machine-readable contract — a clean exit is
//! a run, a non-zero exit is a failure, and token-limit text anywhere in the
//! output is surfaced so the router can fall back.
//!
//! # Construction
//!
//! Built directly from its fields or via [`GenericDriver::new`]. The
//! constructor defaults the prompt to stdin delivery and carries no env; the
//! builder methods refine both.

use std::path::Path;

use async_trait::async_trait;
use tokio::process::Command;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawOutput, apply_user_env, detect_token_limit};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// Runs any configured CLI command as an agent.
///
/// [`command`](GenericDriver::command) is an `argv` vector. When
/// [`stdin_prompt`](GenericDriver::stdin_prompt) is `true` the prompt is fed
/// on the child's stdin; otherwise it is appended as the final positional
/// argument.
#[derive(Debug, Clone, Default)]
pub struct GenericDriver {
    /// The full argv vector used to spawn the child.
    pub command: Vec<String>,
    /// When `true`, the prompt is written to the child's stdin. Otherwise it
    /// is appended as the final positional argument.
    pub stdin_prompt: bool,
    /// User-declared environment variables applied to the child process.
    pub env: Vec<(String, String)>,
}

impl GenericDriver {
    /// Construct a new [`GenericDriver`] with the given argv.
    ///
    /// Defaults: prompt is delivered on stdin, no extra env vars.
    #[must_use]
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            stdin_prompt: true,
            env: Vec::new(),
        }
    }

    /// Toggle whether the prompt should be delivered on stdin (`true`) or
    /// appended as the final argv entry (`false`).
    #[must_use]
    pub fn with_stdin_prompt(mut self, stdin_prompt: bool) -> Self {
        self.stdin_prompt = stdin_prompt;
        self
    }

    /// Append an environment variable to the child's env.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

#[async_trait]
impl AgentDriver for GenericDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        let (program, rest) = self
            .command
            .split_first()
            .ok_or_else(|| AgentError::Launch("agent command is empty".to_owned()))?;
        let mut process = Command::new(program);
        process.current_dir(path);
        process.args(rest);
        // Either the prompt rides stdin, or it becomes the trailing positional
        // argument — never both.
        let stdin = if self.stdin_prompt {
            Some(prompt.as_str().to_owned())
        } else {
            process.arg(prompt.as_str());
            None
        };
        apply_user_env(&mut process, &self.env);
        Ok(AgentCommand {
            process,
            stdin,
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        let raw = RawOutput::from(output);
        // The generic escape hatch has no machine-readable contract: a clean
        // exit is a run, a non-zero exit is a failure. Token-limit text in the
        // output is still surfaced so the router can fall back.
        match raw.exit.into_failure() {
            None => Ok(AgentRun::empty()),
            Some(err) => {
                if let Some(detail) = detect_token_limit(&raw.stdout_str())
                    .or_else(|| detect_token_limit(&raw.stderr_str()))
                {
                    return Err(AgentError::TokenLimit(detail));
                }
                Err(err)
            }
        }
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Generic
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent::Agent;
    use crate::agent::process::RawExit;
    use crate::agent::router::SingleAgentRouter;
    use crate::agent::testutil::{drive, drive_capturing};
    use crate::workspace::{LocalWorkspace, Workspace as _};
    use std::ffi::OsStr;
    use tokio_util::sync::CancellationToken;

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
    fn stdin_prompt_delivers_prompt_on_stdin_not_argv() {
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "cat".into()]);
        let prompt = Prompt::from("from-stdin");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(command.stdin.as_deref(), Some("from-stdin"));
        assert_eq!(command.io, StdioMode::Piped);
        assert_eq!(argv(&command), vec!["-c".to_owned(), "cat".to_owned()]);
    }

    #[test]
    fn inline_prompt_is_appended_as_final_argument() {
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "echo".into()])
            .with_stdin_prompt(false);
        let prompt = Prompt::from("appended-arg");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        assert_eq!(command.stdin, None, "inline delivery must not feed stdin");
        assert_eq!(
            argv(&command),
            vec![
                "-c".to_owned(),
                "echo".to_owned(),
                "appended-arg".to_owned()
            ],
        );
    }

    #[test]
    fn empty_command_is_a_launch_error() {
        let d = GenericDriver::new(Vec::new());
        let prompt = Prompt::from("x");
        let err = d
            .command(Path::new("."), &prompt, None)
            .expect_err("empty argv must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "true".into()])
            .with_env("ITER_TEST_ENV_VAR", "env-value");
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == OsStr::new("ITER_TEST_ENV_VAR") && v == Some(OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    // ----- interpret(): inbound translation --------------------------------

    #[test]
    fn interpret_clean_exit_is_an_empty_run() {
        let d = GenericDriver::new(Vec::new());
        let run = d
            .interpret(&synth_output(RawExit::Code(0), "output", ""))
            .expect("clean exit is a run");
        assert_eq!(run, AgentRun::empty());
    }

    #[test]
    fn interpret_nonzero_exit_is_a_failure() {
        let d = GenericDriver::new(Vec::new());
        let err = d
            .interpret(&synth_output(RawExit::Code(7), "", ""))
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_outranks_the_bare_failure() {
        let d = GenericDriver::new(Vec::new());
        let err = d
            .interpret(&synth_output(
                RawExit::Code(1),
                "",
                "error: context window exceeded",
            ))
            .expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn interpret_signal_termination_survives() {
        let d = GenericDriver::new(Vec::new());
        let err = d
            .interpret(&synth_output(RawExit::Signal(9), "", ""))
            .expect_err("signal is an error");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    #[tokio::test]
    async fn captures_stdout_on_success() {
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "echo hello".into()]);
        let prompt = Prompt::from("ignored");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert!(sink.stdout().await.contains("hello"));
    }

    #[tokio::test]
    async fn stdin_prompt_is_piped_to_child() {
        // `cat` with no args copies stdin to stdout; assert we see the prompt.
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "cat".into()]);
        let prompt = Prompt::from("from-stdin");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert!(sink.stdout().await.contains("from-stdin"));
    }

    #[tokio::test]
    async fn inline_prompt_is_appended_as_arg() {
        // `sh -c 'printf %s "$1"' placeholder <prompt>` — the runtime appends
        // the prompt as the next positional, observable as `$1`.
        let d = GenericDriver::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$1\"".into(),
            "placeholder".into(),
        ])
        .with_stdin_prompt(false);
        let prompt = Prompt::from("appended-arg");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert_eq!(sink.stdout().await, "appended-arg");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let d = GenericDriver::new(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$ITER_TEST_VAR\"".into(),
        ])
        .with_env("ITER_TEST_VAR", "env-value");
        let prompt = Prompt::from("ignored");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert_eq!(sink.stdout().await, "env-value");
    }

    #[tokio::test]
    async fn dsl_env_is_forwarded_to_child() {
        let mut d = GenericDriver::new(vec![
            "sh".into(),
            "-c".into(),
            "printf '%s %s' \"$DSL_VAR_A\" \"$DSL_VAR_B\"".into(),
        ]);
        d.env = vec![
            ("DSL_VAR_A".into(), "alpha".into()),
            ("DSL_VAR_B".into(), "beta".into()),
        ];
        let prompt = Prompt::from("ignored");
        let (result, sink) = drive_capturing(d, Path::new("."), &prompt).await;
        result.expect("run ok");
        assert_eq!(sink.stdout().await, "alpha beta");
    }

    #[tokio::test]
    async fn working_directory_is_applied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "pwd".into()]);
        let prompt = Prompt::from("ignored");
        let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
        result.expect("run ok");
        let out = sink.stdout().await;
        // Resolve the canonical path to avoid symlink mismatches on macOS.
        let canonical = tmp.path().canonicalize().expect("canonicalize");
        assert!(
            out.contains(canonical.to_string_lossy().as_ref()),
            "expected {canonical:?} in {out:?}",
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_termination_is_reported() {
        // Child kills itself with SIGKILL (signal 9). On Unix, the parent
        // should see `AgentError::TerminatedBySignal(9)`.
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "kill -KILL $$".into()]);
        let prompt = Prompt::from("ignored");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("signal is an error");
        assert!(
            matches!(err, AgentError::TerminatedBySignal(9)),
            "got {err:?}",
        );
    }

    /// End-to-end replacement for the old `process.rs`
    /// `raw_process_event_survives_token_limit_classification`: a child that
    /// writes a context-window message to stderr and exits non-zero must
    /// surface as [`AgentError::TokenLimit`] through the whole skeleton, not a
    /// bare failure.
    #[tokio::test]
    async fn token_limit_classification_survives_end_to_end() {
        let d = GenericDriver::new(vec![
            "sh".into(),
            "-c".into(),
            "echo 'context window exceeded' >&2; exit 1".into(),
        ]);
        let prompt = Prompt::from("ignored");
        let err = drive(d, Path::new("."), &prompt)
            .await
            .expect_err("token limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    /// A pre-cancelled token must short-circuit the run before the child is
    /// ever launched. Hand-assembles the drive-equivalent (real
    /// [`LocalWorkspace`] + [`SingleAgentRouter`]) so the cancellation token
    /// can be cancelled up front rather than minted fresh inside `drive`.
    #[tokio::test]
    async fn pre_cancelled_run_is_cancelled() {
        let d = GenericDriver::new(vec!["sh".into(), "-c".into(), "sleep 5".into()]);
        let prompt = Prompt::from("ignored");

        let mut ws = LocalWorkspace::new(Path::new("."));
        // The inherent `setup` returns the concrete active type.
        let active = ws
            .setup(CancellationToken::new())
            .await
            .expect("workspace setup");
        let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(d))));

        let token = CancellationToken::new();
        token.cancel();
        let err = agent
            .run_on(&active, &prompt, token)
            .await
            .expect_err("pre-cancelled run must be cancelled");
        assert!(matches!(err, AgentError::Cancelled), "got {err:?}");
    }
}
