//! The [`Antigravity`] executor and the crate [`Error`] type.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;

use crate::args::ToArgs;
use crate::output::RunOutput;

/// Errors returned while running Antigravity.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// `agy` could not be started or its output could not be collected.
    #[error("antigravity failed: {0}")]
    Io(#[from] std::io::Error),
    /// `agy` exited unsuccessfully. Because the CLI is text-only and its exit
    /// code is overloaded, this is produced only on demand via
    /// [`RunOutput::into_result`](crate::RunOutput::into_result), never by
    /// [`Antigravity::execute`].
    #[error("antigravity exited with code {exit_code:?}: {stderr}")]
    Cli {
        /// `agy` exit code, when one was produced.
        exit_code: Option<i32>,
        /// Standard output decoded lossily as UTF-8.
        stdout: String,
        /// Standard error decoded lossily as UTF-8.
        stderr: String,
    },
}

/// Antigravity executable context.
///
/// This owns the `agy` executable path, working directory, and environment
/// overrides. Command semantics live in command values (`RunCommand`,
/// `PluginCommand`, …).
#[derive(Debug, Clone)]
pub struct Antigravity {
    executable: OsString,
    current_dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
}

impl Antigravity {
    /// Build an executor that invokes a custom `agy` executable.
    #[must_use]
    pub fn new(executable: impl Into<OsString>) -> Self {
        Self {
            executable: executable.into(),
            current_dir: None,
            env: Vec::new(),
        }
    }

    /// `agy` executable name or path.
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

    /// Working directory used when `agy` runs.
    #[must_use]
    pub fn current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Environment overrides used when `agy` runs.
    pub fn envs(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// Set the working directory used when `agy` runs.
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

    /// Add or replace an environment variable used when `agy` runs.
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

    /// Convert a typed Antigravity command into a process builder.
    ///
    /// This exposes the process before it is spawned so callers can apply their
    /// own execution policy, such as sandbox wrapping, process-group handling,
    /// cancellation, or stdout/stderr teeing.
    pub fn to_process<C>(&self, command: &C) -> Command
    where
        C: ToArgs + ?Sized,
    {
        self.process(command.to_args())
    }

    /// Execute an Antigravity command to completion.
    ///
    /// Returns the decoded stdout/stderr and a typed [`Exit`](crate::Exit)
    /// classification. A non-zero exit is *not* an error here: `agy` overloads
    /// its exit code, so the caller decides what a given disposition means (use
    /// [`RunOutput::into_result`](crate::RunOutput::into_result) to opt into
    /// error-on-non-zero).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when `agy` cannot run or its output cannot be
    /// collected.
    pub async fn execute<C>(&self, command: &C) -> Result<RunOutput, Error>
    where
        C: ToArgs + ?Sized,
    {
        let mut process = self.process(command.to_args());
        process.stdin(Stdio::null());
        let output = process.output().await?;
        Ok(RunOutput::from(output))
    }
}

impl Default for Antigravity {
    fn default() -> Self {
        Self::new("agy")
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
