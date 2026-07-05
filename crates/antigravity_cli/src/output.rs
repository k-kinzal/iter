//! Text-only output for `agy`.
//!
//! `agy` has no machine-readable output mode, so a run's "output" is its
//! decoded stdout/stderr plus a typed [`Exit`] classification. There is no
//! event stream or result-object parser: higher-level meaning (auth prompts,
//! token limits, TTY failures) is scanned from the text by the caller.

use std::process::{ExitStatus, Output as ProcessOutput};

use crate::cli::Error;

/// The disposition of a finished `agy` process.
///
/// `agy` overloads its exit code (see the crate README), so this classifies the
/// raw disposition only; it does not decide success/failure meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Exit {
    /// The process exited with code `0`.
    Success,
    /// The process exited with a non-zero code.
    Failure(i32),
    /// The process was terminated by a signal (Unix only).
    Signal(i32),
    /// The process produced neither a code nor a signal.
    Unknown,
}

impl Exit {
    /// Classify a [`std::process::ExitStatus`].
    #[must_use]
    pub fn from_status(status: ExitStatus) -> Self {
        if status.success() {
            return Self::Success;
        }
        if let Some(code) = status.code() {
            return Self::Failure(code);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            if let Some(signal) = status.signal() {
                return Self::Signal(signal);
            }
        }
        Self::Unknown
    }

    /// The exit code, when one was produced.
    #[must_use]
    pub const fn code(self) -> Option<i32> {
        match self {
            Self::Success => Some(0),
            Self::Failure(code) => Some(code),
            Self::Signal(_) | Self::Unknown => None,
        }
    }

    /// Whether the process exited cleanly (code `0`).
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

/// The complete output of a finished `agy` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    stdout: String,
    stderr: String,
    exit: Exit,
}

impl RunOutput {
    /// Standard output, decoded lossily as UTF-8.
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// Standard error, decoded lossily as UTF-8.
    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// The typed exit classification.
    #[must_use]
    pub const fn exit(&self) -> Exit {
        self.exit
    }

    /// Whether the process exited cleanly.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.exit.is_success()
    }

    /// Convert into a `Result`, treating a non-success exit as [`Error::Cli`].
    ///
    /// `agy`'s exit code is overloaded, so callers that need the text even on a
    /// non-zero exit should inspect [`RunOutput`] directly rather than through
    /// this convenience.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cli`] when the process did not exit with code `0`.
    pub fn into_result(self) -> Result<Self, Error> {
        if self.exit.is_success() {
            return Ok(self);
        }
        Err(Error::Cli {
            exit_code: self.exit.code(),
            stdout: self.stdout,
            stderr: self.stderr,
        })
    }
}

impl From<ProcessOutput> for RunOutput {
    fn from(output: ProcessOutput) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit: Exit::from_status(output.status),
        }
    }
}
