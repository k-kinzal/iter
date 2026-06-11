//! macOS [`SandboxBackend`] implementation driven by
//! [`sandbox-exec(1)`](https://keith.github.io/xcode-man-pages/sandbox-exec.1.html).
//!
//! The backend generates a Scheme-dialect profile (`.sb`) in a scratch
//! directory and returns the argv prefix
//! `["sandbox-exec", "-f", "<profile path>"]`. The profile is minimum
//! by construction: default-deny at the top, then the smallest allow-list
//! empirically required to run a Bun/node-based agent (e.g. Claude Code)
//! end-to-end against `api.anthropic.com`.
//!
//! # Minimum profile shape
//!
//! The layout below was narrowed from a Chromium-style enumerated
//! profile by repeated probing against a live `claude --print` invocation
//! (see the `tests/` module for the exact assertions). Every rule in the
//! baseline is load-bearing; dropping it breaks one of:
//!
//! * TLS handshake (needs `com.apple.SecurityServer` mach service for
//!   keychain-backed trust evaluation),
//! * DNS resolution (needs `com.apple.mDNSResponder`),
//! * Bun runtime startup (needs `stat("/")` and terminal ioctl),
//! * outbound TCP to the API (needs `network-outbound`; `network*` was
//!   deliberately rejected as too broad).
//!
//! # Reads
//!
//! File reads are **enumerated**, not blanket-allowed. A blanket
//! `(allow file-read*)` would let the agent read past session logs
//! (`~/.claude/projects/`), other project directories, SSH keys, etc.,
//! defeating the sandbox boundary. Instead:
//!
//! 1. **Blanket metadata** — `(allow file-read-metadata)` lets `stat`,
//!    `lstat`, and directory-component lookup work everywhere. This
//!    reveals file existence and size but never content.
//! 2. **Enumerated data+xattr** — `file-read-data` and `file-read-xattr`
//!    are allowed only on platform system paths, the workspace tmpdir,
//!    agent-declared reads ([`SandboxRequirements::file_reads`]),
//!    resolved `$TMPDIR`, and policy-declared reads
//!    ([`SandboxPolicy::allow_read_outside`]).
//! 3. **Deny overrides** — [`SandboxPolicy::extra_deny_paths`] is
//!    emitted last as `(deny file-read* file-write*)` so explicit
//!    carve-outs (e.g. `~/.ssh`) defeat both metadata and data allows.
//!
//! # Host-level network filtering
//!
//! Apple's built-in sandbox cannot filter by hostname, so
//! [`NetworkAccess::Hosts`](super::super::policy::NetworkAccess::Hosts) is
//! translated to an all-outbound-network allow here. Combining this
//! backend with an external host firewall (`pf`, `Little Snitch`) is the
//! caller's responsibility.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::macos_profile::render_profile;
use super::{BackendError, SandboxBackend, SandboxDescriptor};
use crate::agent::command_path::CommandPath;

/// macOS `sandbox-exec` backend.
#[derive(Debug, Default)]
pub struct SandboxExecBackend {
    /// Scratch directory for the generated `.sb` profile, kept alive
    /// until [`cleanup`](SandboxBackend::cleanup) runs.
    scratch: Option<TempDir>,
    /// Path to the profile file we wrote; returned for tests/diagnostics.
    profile_path: Option<PathBuf>,
}

impl SandboxExecBackend {
    /// Construct a fresh backend. No side effects until
    /// [`prepare`](SandboxBackend::prepare) runs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Path to the generated profile, if [`prepare`](SandboxBackend::prepare)
    /// has been called successfully. Exposed for tests.
    #[must_use]
    pub fn profile_path(&self) -> Option<&Path> {
        self.profile_path.as_deref()
    }
}

impl SandboxBackend for SandboxExecBackend {
    fn name(&self) -> &'static str {
        "sandbox-exec"
    }

    fn prepare(
        &mut self,
        descriptor: &SandboxDescriptor<'_>,
    ) -> Result<Vec<OsString>, BackendError> {
        if CommandPath::resolve("sandbox-exec").is_none() {
            return Err(BackendError::BinaryNotFound("sandbox-exec"));
        }

        let profile = render_profile(descriptor);

        let scratch = TempDir::new()?;
        let profile_path = scratch.path().join("iter-sandbox.sb");
        fs::write(&profile_path, profile)?;

        let prefix = vec![
            OsString::from("sandbox-exec"),
            OsString::from("-f"),
            profile_path.clone().into_os_string(),
        ];
        self.profile_path = Some(profile_path);
        self.scratch = Some(scratch);
        Ok(prefix)
    }

    fn cleanup(&mut self) -> Result<(), BackendError> {
        self.profile_path = None;
        // Drop the TempDir to remove the profile file.
        self.scratch.take();
        Ok(())
    }
}
