use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Output as ProcessOutput, Stdio};

use futures::stream;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;

use crate::agents::Agents;
use crate::args::ToArgs;
use crate::auth::Auth;
use crate::auto_mode::AutoMode;
use crate::command::{Doctor, SetupToken, Update};
use crate::execute::{JsonExecuteCommand, StreamJsonExecuteCommand, TextExecuteCommand};
use crate::install::Install;
use crate::mcp::Mcp;
use crate::output::{JsonOutput, StreamEvent, StreamOutput, TextOutput};
use crate::plugin::Plugin;
use crate::project::Project;
use crate::ultrareview::UltraReview;

/// Errors returned while running Claude Code.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Claude Code could not be started or completed.
    #[error("claude code failed: {0}")]
    Io(#[from] std::io::Error),
    /// Claude Code exited unsuccessfully.
    #[error("claude code exited with code {exit_code:?}: {stderr}")]
    Cli {
        /// Claude Code exit code.
        exit_code: Option<i32>,
        /// Standard output decoded lossily as UTF-8.
        stdout: String,
        /// Standard error decoded lossily as UTF-8.
        stderr: String,
    },
    /// Claude Code emitted malformed JSON for the requested output format.
    #[error("failed to parse claude code JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

/// Incremental reader for `claude --print --output-format stream-json`.
#[derive(Debug)]
struct StreamJsonReader {
    child: Child,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: JoinHandle<Result<Vec<u8>, std::io::Error>>,
}

impl StreamJsonReader {
    /// Read the next non-empty stream-json event line.
    ///
    /// Returns `Ok(None)` after Claude Code closes stdout.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when stdout cannot be read, or [`Error::Json`]
    /// when the next non-empty line is not a Claude Code stream event.
    async fn next_event(&mut self) -> Result<Option<StreamEvent>, Error> {
        loop {
            let Some(line) = self.stdout.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            return serde_json::from_str(&line).map(Some).map_err(Error::from);
        }
    }

    /// Drain remaining stdout and wait for Claude Code to exit successfully.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] while draining or waiting, or [`Error::Cli`] when
    /// Claude Code exits unsuccessfully.
    async fn wait(mut self) -> Result<(), Error> {
        let mut stdout = String::new();
        while let Some(line) = self.stdout.next_line().await? {
            stdout.push_str(&line);
            stdout.push('\n');
        }

        let status = self.child.wait().await?;
        let stderr = self
            .stderr
            .await
            .map_err(|err| Error::Io(std::io::Error::other(err)))??;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Cli {
                exit_code: status.code(),
                stdout,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        }
    }
}

/// A Claude Code command that can be executed to completion.
#[doc(hidden)]
pub trait ExecutableCommand: ToArgs {
    /// Typed result produced by this command.
    type Output: TryFrom<ProcessOutput, Error = Error>;
}

/// Claude Code executable context.
///
/// This owns the Claude executable path, working directory, and environment
/// overrides. Claude Code command semantics and output type live in command
/// values.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    executable: OsString,
    current_dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
}

impl ClaudeCode {
    /// Build an executor that invokes a custom Claude executable.
    #[must_use]
    pub fn new(executable: impl Into<OsString>) -> Self {
        Self {
            executable: executable.into(),
            current_dir: None,
            env: Vec::new(),
        }
    }

    /// Claude executable name or path.
    #[must_use]
    pub fn executable(&self) -> &OsStr {
        &self.executable
    }

    /// Working directory used when Claude Code runs.
    #[must_use]
    pub fn current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Environment overrides used when Claude Code runs.
    pub fn envs(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// Set the working directory used when Claude Code runs.
    pub fn set_current_dir(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(path.into());
        self
    }

    /// Return a copy of this executor with a working directory.
    #[must_use]
    pub fn with_current_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.set_current_dir(path);
        self
    }

    /// Add or replace an environment variable used when Claude Code runs.
    pub fn set_env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        let key = key.into();
        let value = value.into();
        if let Some((_, existing)) = self.env.iter_mut().find(|(existing, _)| existing == &key) {
            *existing = value;
        } else {
            self.env.push((key, value));
        }
        self
    }

    /// Return a copy of this executor with an environment override.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.set_env(key, value);
        self
    }

    /// Remove an environment override from this executor.
    pub fn remove_env(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.env.retain(|(existing, _)| existing != key.as_ref());
        self
    }

    fn process(&self, args: Vec<OsString>) -> Command {
        let mut process = Command::new(&self.executable);
        if let Some(current_dir) = &self.current_dir {
            process.current_dir(current_dir);
        }
        for (key, value) in &self.env {
            process.env(key, value);
        }
        process.args(args);
        process
    }

    /// Convert a typed Claude Code command into a process builder.
    ///
    /// This exposes the process before it is spawned so callers can apply
    /// their own execution policy, such as sandbox wrapping, process-group
    /// handling, cancellation, or stdout/stderr teeing.
    pub fn to_process<C>(&self, command: &C) -> Command
    where
        C: ToArgs + ?Sized,
    {
        self.process(command.to_args())
    }

    /// Execute a Claude Code command to completion.
    ///
    /// Prompt executions choose their result type through
    /// [`ExecuteCommand::text`](crate::ExecuteCommand::text),
    /// [`ExecuteCommand::json`](crate::ExecuteCommand::json), or
    /// [`ExecuteCommand::stream_json`](crate::ExecuteCommand::stream_json).
    /// Claude Code subcommands such as [`Agents`] and [`Mcp`] return stdout as
    /// [`TextOutput`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when Claude Code cannot run or output cannot be
    /// collected.
    /// Command-specific output decoding may also return [`Error::Cli`] or
    /// [`Error::Json`].
    pub async fn execute<C>(&self, command: &C) -> Result<C::Output, Error>
    where
        C: ExecutableCommand + ?Sized,
    {
        let output = self.output(command.to_args()).await?;
        C::Output::try_from(output)
    }

    /// Start Claude Code with `--output-format stream-json`.
    ///
    /// Events are returned as a [`StreamOutput`]. The stream also checks the
    /// final process status after stdout closes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when Claude Code cannot be started.
    pub fn stream(&self, command: &StreamJsonExecuteCommand) -> Result<StreamOutput, Error> {
        let mut process = self.to_process(command);
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = process.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "claude code stream stdout was unavailable",
            ))
        })?;
        let mut stderr = child.stderr.take();
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            if let Some(stderr) = &mut stderr {
                stderr.read_to_end(&mut bytes).await?;
            }
            Ok(bytes)
        });

        let reader = StreamJsonReader {
            child,
            stdout: BufReader::new(stdout).lines(),
            stderr,
        };

        Ok(StreamOutput::from_stream(stream::unfold(
            Some(reader),
            |reader| async {
                let mut reader = reader?;
                match reader.next_event().await {
                    Ok(Some(event)) => Some((Ok(event), Some(reader))),
                    Ok(None) => match reader.wait().await {
                        Ok(()) => None,
                        Err(err) => Some((Err(err), None)),
                    },
                    Err(err) => Some((Err(err), None)),
                }
            },
        )))
    }

    async fn output(&self, args: Vec<OsString>) -> Result<ProcessOutput, Error> {
        let mut process = self.process(args);
        process.stdin(Stdio::null());
        process.output().await.map_err(Error::from)
    }
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self::new("claude")
    }
}

macro_rules! impl_stdout_command {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ExecutableCommand for $ty {
                type Output = TextOutput;
            }
        )+
    };
}

impl_stdout_command!(
    Agents,
    Auth,
    AutoMode,
    Doctor,
    Install,
    Mcp,
    Plugin,
    Project,
    SetupToken,
    UltraReview,
    Update,
);

impl ExecutableCommand for TextExecuteCommand {
    type Output = TextOutput;
}

impl ExecutableCommand for JsonExecuteCommand {
    type Output = JsonOutput;
}

impl ExecutableCommand for StreamJsonExecuteCommand {
    type Output = StreamOutput;
}
