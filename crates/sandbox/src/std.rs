//! Standard-library process command sandboxing.

use std::process::Command as ProcessCommand;

use crate::platform;
use crate::{Error, policy::Policy};

/// Standard-library process command sandboxing extension.
pub trait CommandExt: Sized {
    /// Return a command that runs under `policy`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy cannot be represented by the target
    /// sandbox command.
    fn sandboxed(self, policy: &Policy) -> Result<Command, Error>;
}

impl CommandExt for ProcessCommand {
    fn sandboxed(self, policy: &Policy) -> Result<Command, Error> {
        Command::from_process(self, policy)
    }
}

/// A `std::process::Command` prepared to run inside a sandbox.
#[derive(Debug)]
pub struct Command {
    inner: ProcessCommand,
}

impl Command {
    /// Wrap a process command with `policy`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy cannot be represented by the target
    /// sandbox command.
    pub fn from_process(command: ProcessCommand, policy: &Policy) -> Result<Self, Error> {
        platform::wrap(policy, &command).map(|inner| Self { inner })
    }

    /// Borrow the wrapped process command.
    #[must_use]
    pub fn as_process(&self) -> &ProcessCommand {
        &self.inner
    }

    /// Borrow the wrapped process command mutably.
    pub fn as_process_mut(&mut self) -> &mut ProcessCommand {
        &mut self.inner
    }

    /// Consume this wrapper and return the process command.
    #[must_use]
    pub fn into_process(self) -> ProcessCommand {
        self.inner
    }

    /// Spawn the sandboxed command.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot be spawned.
    pub fn spawn(&mut self) -> std::io::Result<std::process::Child> {
        self.inner.spawn()
    }

    /// Run the sandboxed command to completion and collect output.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot run or output cannot be read.
    pub fn output(&mut self) -> std::io::Result<std::process::Output> {
        self.inner.output()
    }

    /// Run the sandboxed command to completion.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the command cannot run.
    pub fn status(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.inner.status()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn wraps_std_command_with_policy() {
        let policy = Policy::new()
            .allow_read("/usr")
            .clear_environment()
            .set_env("PATH", "/usr/bin")
            .current_dir("/tmp");
        let mut command = ProcessCommand::new("/bin/echo");
        command.arg("hello").env("A", "B");

        let wrapped = command.sandboxed(&policy).expect("wrap");
        let process = wrapped.as_process();

        #[cfg(target_os = "macos")]
        assert_eq!(process.get_program(), "sandbox-exec");
        #[cfg(target_os = "linux")]
        assert_eq!(process.get_program(), "bwrap");
        assert_eq!(process.get_current_dir(), Some(Path::new("/tmp")));
        assert!(
            process
                .get_envs()
                .any(|(key, value)| key == "PATH" && value == Some(OsStr::new("/usr/bin")))
        );
        assert!(
            process
                .get_envs()
                .any(|(key, value)| key == "A" && value == Some(OsStr::new("B")))
        );
    }
}
