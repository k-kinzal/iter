//! Linux [`SandboxBackend`] implementation driven by
//! [`bwrap(1)`](https://man.archlinux.org/man/bwrap.1.en) from the
//! `bubblewrap` project.
//!
//! The backend generates an argv that creates a user-namespace sandbox:
//! private PID namespace, tmpfs-backed `/tmp`, the workspace tmpdir
//! bind-mounted read-write, and everything else read-only by default.
//! `bwrap` does not filter by hostname, so
//! [`NetworkAccess::Hosts`](super::super::policy::NetworkAccess::Hosts)
//! degrades to a shared network namespace; combine with an external
//! firewall (`iptables`, `nftables`) when host-level filtering is
//! required.
//!
//! # Untested host note
//!
//! This backend is implemented to specification but was authored on a
//! macOS host. Validate before production use — at minimum run the
//! `iter_core::workspace` integration tests (`--ignored`) on a Linux host
//! with `bubblewrap` installed.

use std::ffi::OsString;

use super::linux_argv::render_argv;
use super::{BackendError, SandboxBackend, SandboxDescriptor, which_sync};

/// Linux `bwrap` backend.
#[derive(Debug, Default)]
pub struct BwrapBackend;

impl BwrapBackend {
    /// Construct a fresh backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl SandboxBackend for BwrapBackend {
    fn name(&self) -> &'static str {
        "bwrap"
    }

    fn prepare(
        &mut self,
        descriptor: &SandboxDescriptor<'_>,
    ) -> Result<Vec<OsString>, BackendError> {
        if which_sync("bwrap").is_none() {
            return Err(BackendError::BinaryNotFound("bwrap"));
        }
        Ok(render_argv(descriptor))
    }
}
