//! Tokio process command sandboxing.

use tokio::process::Command as TokioProcessCommand;

use crate::Error;
use crate::policy::Policy;

/// Tokio process command sandboxing extension.
pub trait CommandExt: Sized {
    /// Return a Tokio command that runs under `policy`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy cannot be represented by the target
    /// sandbox command.
    fn sandboxed(self, policy: &Policy) -> Result<Command, Error>;
}

impl CommandExt for TokioProcessCommand {
    fn sandboxed(self, policy: &Policy) -> Result<Command, Error> {
        Command::from_process(self, policy)
    }
}

/// A `tokio::process::Command` prepared to run inside a sandbox.
#[derive(Debug)]
pub struct Command {
    inner: TokioProcessCommand,
}

impl Command {
    /// Wrap a Tokio process command with `policy`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy cannot be represented by the target
    /// sandbox command.
    pub fn from_process(command: TokioProcessCommand, policy: &Policy) -> Result<Self, Error> {
        crate::std::Command::from_process(command.into_std(), policy).map(|command| Self {
            inner: TokioProcessCommand::from(command.into_process()),
        })
    }

    /// Borrow the wrapped Tokio process command.
    #[must_use]
    pub fn as_process(&self) -> &TokioProcessCommand {
        &self.inner
    }

    /// Borrow the wrapped Tokio process command mutably.
    pub fn as_process_mut(&mut self) -> &mut TokioProcessCommand {
        &mut self.inner
    }

    /// Consume this wrapper and return the Tokio process command.
    #[must_use]
    pub fn into_process(self) -> TokioProcessCommand {
        self.inner
    }

    /// Spawn the sandboxed command.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot be spawned.
    pub fn spawn(&mut self) -> std::io::Result<tokio::process::Child> {
        self.inner.spawn()
    }

    /// Run the sandboxed command to completion and collect output.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot run or output cannot be read.
    pub async fn output(&mut self) -> std::io::Result<std::process::Output> {
        self.inner.output().await
    }

    /// Run the sandboxed command to completion.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot run.
    pub async fn status(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.status().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_tokio_command_with_policy() {
        let policy = Policy::new().allow_read("/usr");
        let mut command = TokioProcessCommand::new("/bin/echo");
        command.arg("hello");

        let wrapped = command.sandboxed(&policy).expect("wrap");
        let process = wrapped.as_process().as_std();

        #[cfg(target_os = "macos")]
        assert_eq!(process.get_program(), "sandbox-exec");
        #[cfg(target_os = "linux")]
        assert_eq!(process.get_program(), "bwrap");
    }
}
