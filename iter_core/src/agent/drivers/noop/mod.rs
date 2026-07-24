//! [`NoopDriver`] — the trivial `sh` agent that does nothing and exits `0`.
//!
//! Once a special-cased in-process agent that returned an empty
//! [`AgentRun`] without ever leaving the process, the no-op is now an
//! ordinary [`AgentDriver`]: it translates to `sh -c 'exit 0'`. That single
//! change is deliberate — iter no longer bends the [`Agent`](crate::agent::Agent)
//! cycle around a fake that skips it. The trivial child is spawned on the
//! workspace, its (empty) stdout/stderr are teed through the sink,
//! cancellation and the sandbox wrap apply exactly as they do for a real
//! CLI, and its exit status is interpreted the same way. The no-op is now a
//! genuine end-to-end exercise of the spawn/tee/cancel/sandbox path with the
//! agent's own behaviour reduced to nothing.
//!
//! Useful for verifying workspace setup/teardown, event-handler
//! registration, runner overhead benchmarks, and dry-running a declaration —
//! now with the full child-process machinery in the loop.

use std::path::Path;

use async_trait::async_trait;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::RawOutput;
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// Agent that translates to a shell no-op (`sh -c 'exit 0'`).
///
/// The prompt and session token are ignored; the child writes nothing and
/// exits cleanly, so [`interpret`](AgentDriver::interpret) always yields an
/// empty [`AgentRun`].
#[derive(Debug, Clone)]
pub struct NoopDriver;

#[async_trait]
impl AgentDriver for NoopDriver {
    fn command(
        &self,
        _path: &Path,
        _prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        let mut process = tokio::process::Command::new("sh");
        process.arg("-c").arg("exit 0");
        Ok(AgentCommand {
            process,
            stdin: None,
            io: StdioMode::Piped,
            temporary_files: Vec::new(),
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // The child has no in-band verdict: a clean exit is a run, anything
        // else is the faithful failure the platform reported.
        match RawOutput::from(output).exit.into_failure() {
            None => Ok(AgentRun::empty()),
            Some(err) => Err(err),
        }
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Noop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::drive;
    use tempfile::TempDir;

    #[test]
    fn command_is_a_trivial_success_shell() {
        let prompt = Prompt::from("ignored");
        let command = NoopDriver
            .command(Path::new("."), &prompt, None)
            .expect("command");
        let std = command.process.as_std();
        assert_eq!(std.get_program(), "sh");
        let args: Vec<String> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-c".to_owned(), "exit 0".to_owned()]);
        assert_eq!(command.stdin, None);
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[tokio::test]
    async fn drives_to_success_through_the_skeleton() {
        let tmp = TempDir::new().expect("tempdir");
        let prompt = Prompt::from("ignored");
        let run = drive(NoopDriver, tmp.path(), &prompt)
            .await
            .expect("run ok");
        assert_eq!(run.session_id, None);
    }

    #[tokio::test]
    async fn leaves_the_workspace_untouched() {
        let tmp = TempDir::new().expect("tempdir");
        let before: Vec<_> = std::fs::read_dir(tmp.path()).expect("read_dir").collect();
        assert!(before.is_empty());

        let prompt = Prompt::from("ignored");
        drive(NoopDriver, tmp.path(), &prompt)
            .await
            .expect("run ok");

        let after: Vec<_> = std::fs::read_dir(tmp.path()).expect("read_dir").collect();
        assert!(after.is_empty(), "the no-op child must write nothing");
    }
}
