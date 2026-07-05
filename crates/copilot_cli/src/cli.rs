//! The [`Copilot`] executor and the crate [`Error`] type.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Output as ProcessOutput, Stdio};

use futures::stream;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;

use crate::args::ToArgs;
use crate::output::{Event, EventStream};
use crate::run::JsonRunCommand;

/// Errors returned while running Copilot.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Copilot could not be started or its output could not be collected.
    #[error("copilot failed: {0}")]
    Io(#[from] std::io::Error),
    /// Copilot exited unsuccessfully without usable machine-readable output.
    #[error("copilot exited with code {exit_code:?}: {stderr}")]
    Cli {
        /// Copilot exit code.
        exit_code: Option<i32>,
        /// Standard output decoded lossily as UTF-8.
        stdout: String,
        /// Standard error decoded lossily as UTF-8.
        stderr: String,
    },
    /// Copilot emitted malformed JSON for the requested output format.
    #[error("failed to parse copilot JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

/// Incremental reader for `copilot --output-format json`.
#[derive(Debug)]
struct EventReader {
    child: Child,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: JoinHandle<Result<Vec<u8>, std::io::Error>>,
}

impl EventReader {
    /// Read the next JSON-object event line, skipping blank and non-object
    /// lines. Returns `Ok(None)` after Copilot closes stdout.
    async fn next_event(&mut self) -> Result<Option<Event>, Error> {
        loop {
            let Some(line) = self.stdout.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if value.is_object() {
                    return Ok(Some(Event::from_value(value)));
                }
            }
        }
    }

    /// Drain remaining stdout and wait for Copilot to exit successfully.
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

/// A Copilot command that can be executed to completion.
#[doc(hidden)]
pub trait ExecutableCommand: ToArgs {
    /// Typed result produced by this command.
    type Output: TryFrom<ProcessOutput, Error = Error>;
}

/// Copilot executable context.
///
/// This owns the Copilot executable path, working directory, and environment
/// overrides. Command semantics and output type live in command values.
#[derive(Debug, Clone)]
pub struct Copilot {
    executable: OsString,
    current_dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
}

impl Copilot {
    /// Build an executor that invokes a custom Copilot executable.
    #[must_use]
    pub fn new(executable: impl Into<OsString>) -> Self {
        Self {
            executable: executable.into(),
            current_dir: None,
            env: Vec::new(),
        }
    }

    /// Copilot executable name or path.
    #[must_use]
    pub fn executable(&self) -> &OsStr {
        &self.executable
    }

    /// Files that must be readable to launch this executable.
    ///
    /// Returns the resolved path found through `$PATH` (or the configured
    /// absolute/relative path) plus the canonical target when the executable is
    /// a symlink shim.
    #[must_use]
    pub fn executable_read_paths(&self) -> Vec<PathBuf> {
        executable_read_paths(&self.executable)
    }

    /// Working directory used when Copilot runs.
    #[must_use]
    pub fn current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Environment overrides used when Copilot runs.
    pub fn envs(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// Set the working directory used when Copilot runs.
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

    /// Add or replace an environment variable used when Copilot runs.
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

    /// Convert a typed Copilot command into a process builder.
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

    /// Execute a Copilot command to completion.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when Copilot cannot run or output cannot be
    /// collected. Command-specific output decoding may also return
    /// [`Error::Cli`] or [`Error::Json`].
    pub async fn execute<C>(&self, command: &C) -> Result<C::Output, Error>
    where
        C: ExecutableCommand + ?Sized,
    {
        let output = self.output(command.to_args()).await?;
        C::Output::try_from(output)
    }

    /// Start `copilot --output-format json` and read its events incrementally.
    ///
    /// Events are returned as an [`EventStream`]. The stream also checks the
    /// final process status after stdout closes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when Copilot cannot be started.
    pub fn stream(&self, command: &JsonRunCommand) -> Result<EventStream, Error> {
        let mut process = self.to_process(command);
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = process.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "copilot stream stdout was unavailable",
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

        let reader = EventReader {
            child,
            stdout: BufReader::new(stdout).lines(),
            stderr,
        };

        Ok(EventStream::from_stream(stream::unfold(
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

impl Default for Copilot {
    fn default() -> Self {
        Self::new("copilot")
    }
}

fn executable_read_paths(executable: &OsStr) -> Vec<PathBuf> {
    let Some(resolved) = resolve_executable(executable) else {
        return Vec::new();
    };

    let canonical = std::fs::canonicalize(&resolved)
        .ok()
        .filter(|path| path != &resolved);
    let mut out = vec![resolved];
    if let Some(path) = canonical {
        out.push(path);
    }
    out
}

fn resolve_executable(executable: &OsStr) -> Option<PathBuf> {
    let command = Path::new(executable);
    if is_path_like(command) {
        if command.is_file() {
            return Some(command.to_path_buf());
        }
        return None;
    }

    let path_env = std::env::var_os("PATH")?;
    std::env::split_paths(&path_env)
        .map(|entry| entry.join(command))
        .find(|candidate| candidate.is_file())
}

fn is_path_like(command: &Path) -> bool {
    command.is_absolute() || command.components().count() > 1
}

impl ExecutableCommand for JsonRunCommand {
    type Output = crate::output::RunOutput;
}
