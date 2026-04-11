//! [`SandboxBackend`] — OS-specific sandbox driver.
//!
//! A backend is responsible for translating a merged
//! [`SandboxDescriptor`] into:
//!
//! - Any transient on-disk artefacts (e.g. a
//!   [macOS sandbox-exec profile](macos)).
//! - The argv prefix child processes must be wrapped with to enter the
//!   sandbox.
//!
//! The workspace invokes [`prepare`](SandboxBackend::prepare) during
//! [`SandboxWorkspace::setup`](super::SandboxWorkspace::setup) and
//! [`cleanup`](SandboxBackend::cleanup) during
//! [`SandboxWorkspace::teardown`](super::SandboxWorkspace::teardown).

pub mod linux;
pub mod linux_argv;
pub mod macos;
pub mod macos_profile;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::SandboxRequirements;
use thiserror::Error;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use super::error::SandboxWorkspaceError;
use super::policy::SandboxPolicy;

/// Merged view passed to a backend when preparing a sandbox.
#[derive(Debug)]
pub struct SandboxDescriptor<'a> {
    /// The tmpdir the workspace lives in — this is the root the agent
    /// operates in and is always readable/writable inside the sandbox.
    pub workspace_path: &'a Path,
    /// Workspace-level upper-bound rules.
    pub policy: &'a SandboxPolicy,
    /// Agent-declared minimum needs.
    pub requirements: &'a SandboxRequirements,
}

/// Errors produced by a [`SandboxBackend`].
#[derive(Debug, Error)]
pub enum BackendError {
    /// The backend required a binary (e.g. `sandbox-exec`, `bwrap`) that
    /// is not present on `PATH`.
    #[error("sandbox backend binary not found: {0}")]
    BinaryNotFound(&'static str),
    /// The host OS is not supported by any built-in backend.
    #[error("no sandbox backend for this platform")]
    UnsupportedPlatform,
    /// An I/O error occurred while preparing or tearing down the backend.
    #[error("sandbox backend I/O error: {0}")]
    Io(#[from] io::Error),
    /// The requested policy is incompatible with the backend's
    /// capabilities.
    #[error("sandbox policy unsupported by backend: {0}")]
    PolicyUnsupported(String),
}

/// OS-specific driver producing the argv prefix that wraps the agent's
/// child processes.
pub trait SandboxBackend: Send + Sync + std::fmt::Debug {
    /// Build any artefacts the backend needs and return the argv prefix
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// that callers must prepend to every command spawned inside the
    /// sandbox.
    ///
    /// For example, a macOS backend returns
    /// `["sandbox-exec", "-f", "/tmp/.../profile.sb"]`; a Linux `bwrap`
    /// backend returns the full `bwrap --bind ... --` argv.
    fn prepare(
        &mut self,
        descriptor: &SandboxDescriptor<'_>,
    ) -> Result<Vec<OsString>, BackendError>;

    /// Clean up any artefacts created by [`prepare`](Self::prepare). The
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// default implementation is a no-op.
    fn cleanup(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Human-readable backend name for diagnostics — e.g. `"sandbox-exec"`
    /// or `"bwrap"`. Only used in log messages.
    fn name(&self) -> &'static str;
}

/// Construct the platform-default backend on macOS or Linux.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn build_backend() -> Box<dyn SandboxBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::SandboxExecBackend::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::BwrapBackend::new())
    }
}

/// Stub for hosts without a built-in driver — always returns
/// [`SandboxWorkspaceError::UnsupportedPlatform`].
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn build_backend() -> Result<Box<dyn SandboxBackend>, SandboxWorkspaceError> {
    Err(SandboxWorkspaceError::UnsupportedPlatform)
}

/// Returns `true` if a sandbox backend is available for the host
/// platform and its driver binary is present on `PATH`.
#[must_use]
pub(super) fn detect_backend_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        which_sync("sandbox-exec").is_some()
    }
    #[cfg(target_os = "linux")]
    {
        which_sync("bwrap").is_some()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Synchronous `which` used by the backends and by
/// [`detect_backend_available`]. Returns the first matching path on
/// `PATH` or `None` when the binary is missing.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn which_sync(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
