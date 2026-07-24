//! [`FakeDriver`] — a configurable fake agent synthesised into an `sh` script.
//!
//! Once an in-process agent that wrote files, pushed lines to the
//! [`OutputSink`](crate::log::OutputSink), and returned an
//! [`AgentRun`]/[`AgentError`] directly, the fake is now an ordinary
//! [`AgentDriver`]: its configuration is compiled into a trivial shell
//! script that reproduces the same observable effects through a real child
//! process. iter no longer bends the [`Agent`](crate::agent::Agent) cycle
//! around a fake that skips it — the deterministic file changes, stdout /
//! stderr, delay, and exit status all flow through the exact
//! spawn/tee/cancel/sandbox path a real CLI takes. That is the point of the
//! fake: exercise the infrastructure without an external agent binary.
//!
//! # Script synthesis
//!
//! [`command`](AgentDriver::command) emits `sh -c <script>`. Every
//! configured value crosses into the script through an **environment
//! variable**, never by string interpolation into the script body, so file
//! bodies and output lines containing quotes, `$`, newlines, or other shell
//! metacharacters cannot break out of their intended slot. The script only
//! references those variables (`printf '%s' "$VAR"`), leaving all quoting to
//! the shell.
//!
//! File paths are written relative to the workspace working directory, which
//! [`ActiveWorkspace::spawn`](crate::workspace::ActiveWorkspace::spawn)
//! imposes as the child's cwd — the driver never resolves them itself.
//!
//! # Semantic narrowing from the in-process fake
//!
//! The in-process fake could report any [`i32`] exit code verbatim. Routed
//! through `sh -c '… exit N'`, the code is now subject to the shell's own
//! `exit` coercion: only `0..=255` is meaningful, and values outside that
//! range (including negatives) are wrapped or rejected by the shell rather
//! than surfaced faithfully. Configurations that relied on an exotic exit
//! code no longer see it round-trip; realistic small non-negative codes are
//! unaffected. The public [`exit_code`](FakeDriver::exit_code) field keeps
//! its `i32` type to match the language-layer `agent fake { exit_code = N }`
//! binding.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path};

use async_trait::async_trait;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::RawOutput;
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// Configurable fake agent for verification testing.
///
/// Produces deterministic file changes, stdout / stderr output, and an exit
/// status through the real child-process pipeline without requiring a real
/// AI agent binary. Its fields mirror the language-layer
/// `agent fake { … }` binding one-to-one.
#[derive(Debug, Clone)]
pub struct FakeDriver {
    /// Process exit code. `0` = success, non-zero = failure. Coerced to
    /// `0..=255` by the shell's `exit` (see the [module docs](self)).
    pub exit_code: i32,
    /// Simulated execution delay in seconds, emitted as `sleep N`. `0` =
    /// immediate.
    pub delay_secs: u64,
    /// Lines written to the child's stdout (each newline-terminated), teed
    /// through the [`OutputSink`](crate::log::OutputSink).
    pub stdout: Vec<String>,
    /// Lines written to the child's stderr (each newline-terminated), teed
    /// through the [`OutputSink`](crate::log::OutputSink).
    pub stderr: Vec<String>,
    /// Files to create/overwrite in the workspace directory, keyed by a path
    /// relative to the workspace cwd (no absolute path, no `..` component).
    pub files: BTreeMap<String, String>,
}

impl FakeDriver {
    /// Reject any file key that could escape the workspace working directory.
    ///
    /// Runs inside [`command`](AgentDriver::command) so a bad configuration
    /// is caught as an [`AgentError::Launch`] before the child is spawned,
    /// preserving the in-process fake's up-front validation.
    fn validate_path(rel: &str) -> Result<(), AgentError> {
        let path = Path::new(rel);
        if path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return Err(AgentError::Launch(format!(
                "fake agent file path must be relative without `..`: {rel}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentDriver for FakeDriver {
    fn command(
        &self,
        _path: &Path,
        _prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // `set -e` makes a failed file write (or any step) abort with a
        // non-zero status the interpreter reads as a failure; `set -u`
        // guards against a mistyped variable name silently expanding to
        // nothing.
        let mut script = String::from("set -eu\n");
        let mut process = tokio::process::Command::new("sh");

        for (i, (rel, body)) in self.files.iter().enumerate() {
            Self::validate_path(rel)?;
            process.env(format!("ITER_FAKE_PATH_{i}"), rel);
            process.env(format!("ITER_FAKE_BODY_{i}"), body);
            // Recreate the file relative to the cwd the workspace imposes:
            // derive its parent, create it unless trivial, then write the
            // exact body with no trailing newline (`printf '%s'`).
            let _ = writeln!(
                script,
                "d=$(dirname \"$ITER_FAKE_PATH_{i}\")\n\
                 [ \"$d\" = \".\" ] || mkdir -p \"$d\"\n\
                 printf '%s' \"$ITER_FAKE_BODY_{i}\" > \"$ITER_FAKE_PATH_{i}\""
            );
        }

        if !self.stdout.is_empty() {
            // One env var holds all lines joined by newlines; `printf '%s\n'`
            // over that single argument emits each line newline-terminated.
            process.env("ITER_FAKE_STDOUT", self.stdout.join("\n"));
            script.push_str("printf '%s\\n' \"$ITER_FAKE_STDOUT\"\n");
        }
        if !self.stderr.is_empty() {
            process.env("ITER_FAKE_STDERR", self.stderr.join("\n"));
            script.push_str("printf '%s\\n' \"$ITER_FAKE_STDERR\" 1>&2\n");
        }
        if self.delay_secs > 0 {
            let _ = writeln!(script, "sleep {}", self.delay_secs);
        }
        let _ = writeln!(script, "exit {}", self.exit_code);

        process.arg("-c").arg(script);
        Ok(AgentCommand {
            process,
            stdin: None,
            io: StdioMode::Piped,
            temporary_files: Vec::new(),
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // No in-band verdict: the shell's exit is the whole truth. A clean
        // exit is an (empty) run; a non-zero code becomes `Failed`, a signal
        // becomes `TerminatedBySignal`.
        match RawOutput::from(output).exit.into_failure() {
            None => Ok(AgentRun::empty()
                .with_text_output(String::from_utf8_lossy(&output.stdout).into_owned())),
            Some(err) => Err(err),
        }
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Fake
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::agent::router::SingleAgentRouter;
    use crate::agent::testutil::{drive, drive_capturing};
    use crate::workspace::{LocalWorkspace, Workspace as _};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    /// A fake with every effect switched off — the empty configuration.
    fn fake() -> FakeDriver {
        FakeDriver {
            exit_code: 0,
            delay_secs: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
            files: BTreeMap::new(),
        }
    }

    // ----- command(): path validation happens before spawn -----------------

    #[test]
    fn absolute_file_path_is_a_launch_error() {
        let mut files = BTreeMap::new();
        files.insert("/etc/passwd".to_owned(), "bad".to_owned());
        let driver = FakeDriver { files, ..fake() };
        let err = driver
            .command(Path::new("."), &Prompt::from("x"), None)
            .expect_err("absolute path must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[test]
    fn parent_dir_traversal_is_a_launch_error() {
        let mut files = BTreeMap::new();
        files.insert("../../escape.txt".to_owned(), "bad".to_owned());
        let driver = FakeDriver { files, ..fake() };
        let err = driver
            .command(Path::new("."), &Prompt::from("x"), None)
            .expect_err("parent traversal must fail");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    // ----- through the full skeleton ----------------------------------------

    #[tokio::test]
    async fn empty_config_succeeds_like_a_noop() {
        let tmp = TempDir::new().expect("tempdir");
        let prompt = Prompt::from("ignored");
        let run = drive(fake(), tmp.path(), &prompt).await.expect("run ok");
        assert_eq!(run.session_id, None);
    }

    #[tokio::test]
    async fn files_are_written_into_the_workspace() {
        let tmp = TempDir::new().expect("tempdir");
        let mut files = BTreeMap::new();
        files.insert("output/result.txt".to_owned(), "content-a".to_owned());
        files.insert("nested/deep/file.txt".to_owned(), "content-b".to_owned());
        let driver = FakeDriver { files, ..fake() };
        let prompt = Prompt::from("ignored");
        drive(driver, tmp.path(), &prompt).await.expect("run ok");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("output/result.txt")).expect("read a"),
            "content-a",
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("nested/deep/file.txt")).expect("read b"),
            "content-b",
        );
    }

    #[tokio::test]
    async fn zero_exit_code_is_success() {
        let tmp = TempDir::new().expect("tempdir");
        let driver = FakeDriver {
            exit_code: 0,
            ..fake()
        };
        let prompt = Prompt::from("ignored");
        drive(driver, tmp.path(), &prompt).await.expect("run ok");
    }

    #[tokio::test]
    async fn nonzero_exit_code_maps_to_failed() {
        let tmp = TempDir::new().expect("tempdir");
        let driver = FakeDriver {
            exit_code: 7,
            ..fake()
        };
        let prompt = Prompt::from("ignored");
        let err = drive(driver, tmp.path(), &prompt)
            .await
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn stdout_and_stderr_reach_the_sink_line_by_line() {
        let tmp = TempDir::new().expect("tempdir");
        let driver = FakeDriver {
            stdout: vec!["hello".to_owned(), "world".to_owned()],
            stderr: vec!["warn".to_owned()],
            ..fake()
        };
        let prompt = Prompt::from("ignored");
        let (result, sink) = drive_capturing(driver, tmp.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(
            run.output,
            Some(crate::agent::AgentOutput::Text("hello\nworld\n".into()))
        );
        assert_eq!(sink.stdout().await, "hello\nworld\n");
        assert_eq!(sink.stderr().await, "warn\n");
    }

    // ----- cancellation, hand-assembled to control the token ---------------

    /// Set up a real local workspace over a fresh tempdir. The `TempDir`
    /// guard is returned so the caller keeps it alive for the run.
    async fn active_over(tmp: &TempDir) -> Box<dyn crate::workspace::ActiveWorkspace> {
        // Call through the trait: the inherent `setup` returns the
        // concrete active type.
        crate::workspace::Workspace::setup(
            &mut LocalWorkspace::new(tmp.path()),
            CancellationToken::new(),
        )
        .await
        .expect("workspace setup")
    }

    #[tokio::test]
    async fn a_pre_cancelled_token_yields_cancelled() {
        let tmp = TempDir::new().expect("tempdir");
        let active = active_over(&tmp).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(FakeDriver {
            delay_secs: 3600,
            ..fake()
        }))));
        let prompt = Prompt::from("ignored");
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            agent.run_on(&*active, &prompt, cancel),
        )
        .await
        .expect("run must not hang");
        assert!(
            matches!(result, Err(AgentError::Cancelled)),
            "got {result:?}"
        );
    }

    #[tokio::test]
    async fn a_long_delay_is_interrupted_by_cancellation() {
        let tmp = TempDir::new().expect("tempdir");
        let active = active_over(&tmp).await;
        let cancel = CancellationToken::new();
        let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(FakeDriver {
            delay_secs: 3600,
            ..fake()
        }))));
        let prompt = Prompt::from("ignored");

        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            agent.run_on(&*active, &prompt, cancel),
        )
        .await
        .expect("cancellation must terminate the sleeping child promptly");
        assert!(
            matches!(result, Err(AgentError::Cancelled)),
            "got {result:?}"
        );
    }
}
