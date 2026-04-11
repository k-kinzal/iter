//! [`SandboxWorkspace`] — [`Workspace`] implementation that clones into
//! a tmpdir and wraps agent commands in a kernel-level sandbox.
//!
//! See the [module docs](super) for the conceptual model — the
//! "clone + wrap" pipeline, the upper/lower bound contract, and the
//! platform support matrix.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{ITER_SANDBOX_COMMAND_PREFIX, SandboxRequirements, Workspace, encode_prefix_env};
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::workspace::apply_back::ApplyBackMode;
use crate::workspace::clone::CloneSettings;
use crate::workspace::mirror::{CloneFilter, Mirror};

use super::backend::{SandboxBackend, SandboxDescriptor, build_backend, detect_backend_available};
use super::error::SandboxWorkspaceError;
use super::policy::SandboxPolicy;

/// Workspace that clones the base directory into a tmpdir and wraps
/// agent commands in a kernel-level sandbox.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::{SandboxRequirements, Workspace};
/// use iter_core::workspace::{
///     ApplyBackMode, CloneSettings, NetworkAccess, SandboxPolicy, SandboxWorkspace,
/// };
/// use tokio_util::sync::CancellationToken;
///
/// let mut ws = SandboxWorkspace::new(
///     "/Users/me/my-project",
///     CloneSettings {
///         excludes: vec!["scratch".into()],
///         includes: Vec::new(),
///         preserve_mtime: false,
///         apply_back: ApplyBackMode::Sync,
///         apply_back_excludes: Vec::new(),
///         apply_back_includes: Vec::new(),
///     },
///     SandboxPolicy {
///         network: NetworkAccess::Hosts(vec!["api.anthropic.com".into()]),
///         allow_read_outside: Vec::new(),
///         allow_write_outside: Vec::new(),
///         extra_deny_paths: Vec::new(),
///         allow_exec: Vec::new(),
///     },
///     SandboxRequirements {
///         network_hosts: vec!["api.anthropic.com".into()],
///         env_pass: vec!["CLAUDE_*".into()],
///         ..Default::default()
///     },
/// );
/// ws.setup(CancellationToken::new()).await?;
/// // ... run the agent ...
/// ws.teardown(CancellationToken::new()).await?;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct SandboxWorkspace {
    base: PathBuf,
    settings: CloneSettings,
    policy: SandboxPolicy,
    requirements: SandboxRequirements,

    mirror: Option<Mirror>,
    backend: Option<Box<dyn SandboxBackend>>,
    command_prefix: Vec<OsString>,
    set_up: bool,
}

impl SandboxWorkspace {
    /// Create a new [`SandboxWorkspace`] rooted at `base`.
    ///
    /// Every knob is supplied by the caller. `settings` controls the
    /// clone-layer behaviour (mirrors [`CloneSettings`]). `policy` is the
    /// project's upper-bound rule set from the Iterfile. `requirements`
    /// is the agent's lower-bound declaration shipped by iter.
    #[must_use]
    pub fn new(
        base: impl Into<PathBuf>,
        settings: CloneSettings,
        policy: SandboxPolicy,
        requirements: SandboxRequirements,
    ) -> Self {
        Self {
            base: base.into(),
            settings,
            policy,
            requirements,
            mirror: None,
            backend: None,
            command_prefix: Vec::new(),
            set_up: false,
        }
    }

    /// Returns `true` if the workspace has been successfully set up.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.set_up
    }

    /// The current apply-back mode.
    #[must_use]
    pub fn apply_back_mode(&self) -> ApplyBackMode {
        self.settings.apply_back
    }

    /// The argv prefix the sandbox backend produced during setup.
    ///
    /// Empty before [`setup`](Workspace::setup) is called; populated by
    /// the backend once the workspace is active. Useful for tests and
    /// for callers that want to spawn commands inside the sandbox
    /// without touching [`ITER_SANDBOX_COMMAND_PREFIX`].
    #[must_use]
    pub fn command_prefix(&self) -> &[OsString] {
        &self.command_prefix
    }

    /// Returns `true` if a sandbox backend is available for the host
    /// platform and its driver binary is present on `PATH`.
    ///
    /// Intended for tests and friendly CLI diagnostics — use it to skip
    /// when the host can't enforce a sandbox.
    #[must_use]
    pub fn detect_backend_available() -> bool {
        detect_backend_available()
    }
}

impl Workspace for SandboxWorkspace {
    type Error = SandboxWorkspaceError;

    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), Self::Error> {
        if cancel.is_cancelled() {
            return Err(SandboxWorkspaceError::Cancelled);
        }

        // ----- Phase 1: clone base into tmpdir ---------------------------
        let meta = match fs::metadata(&self.base).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SandboxWorkspaceError::NotFound(self.base.clone()));
            }
            Err(e) => return Err(SandboxWorkspaceError::Io(e)),
        };
        if !meta.is_dir() {
            return Err(SandboxWorkspaceError::NotADirectory(self.base.clone()));
        }

        let clone_filter = CloneFilter::compile(&self.settings.excludes, &self.settings.includes)?;
        let apply_back_filter = self.settings.apply_back_filter()?;
        let mirror = Mirror::materialize(
            self.base.clone(),
            &clone_filter,
            apply_back_filter,
            self.settings.preserve_mtime,
        )
        .await?;
        let temp_path = mirror.path().to_path_buf();

        // ----- Phase 2: prepare sandbox backend --------------------------
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let mut backend = build_backend();
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let mut backend = build_backend()?;
        let descriptor = SandboxDescriptor {
            workspace_path: &temp_path,
            policy: &self.policy,
            requirements: &self.requirements,
        };
        let prefix = backend.prepare(&descriptor)?;

        // ----- Phase 3: publish prefix as env var ------------------------
        // SAFETY: mutating process env is unsound only when other threads
        // are concurrently reading it. The runner is single-instance and
        // children spawn after this point, so the write happens-before
        // their env snapshot. See `ITER_SANDBOX_COMMAND_PREFIX` docs for
        // the consumer contract.
        let encoded = encode_prefix_env(&prefix);
        unsafe {
            std::env::set_var(ITER_SANDBOX_COMMAND_PREFIX, &encoded);
        }

        self.mirror = Some(mirror);
        self.backend = Some(backend);
        self.command_prefix = prefix;
        self.set_up = true;
        tracing::debug!(
            base = %self.base.display(),
            temp = %temp_path.display(),
            mode = ?self.settings.apply_back,
            prefix = ?self.command_prefix,
            "sandbox workspace set up",
        );
        Ok(())
    }

    async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
        if !self.set_up {
            return Ok(());
        }

        // Apply-back BEFORE tearing down the sandbox artefacts so a
        // backend cleanup failure doesn't strand the agent's work.
        if let Some(mirror) = self.mirror.as_ref() {
            match self.settings.apply_back {
                ApplyBackMode::Discard => {}
                ApplyBackMode::Sync => mirror.sync_back().await?,
                ApplyBackMode::Merge => mirror.merge_back().await?,
            }
        }

        // Unset the env var we exported at setup; see SAFETY note there.
        unsafe {
            std::env::remove_var(ITER_SANDBOX_COMMAND_PREFIX);
        }

        if let Some(mut backend) = self.backend.take() {
            backend.cleanup()?;
        }

        if let Some(mirror) = self.mirror.take() {
            mirror.close().await?;
        }
        self.command_prefix.clear();
        self.set_up = false;
        tracing::debug!(base = %self.base.display(), "sandbox workspace torn down");
        Ok(())
    }

    fn path(&self) -> &Path {
        // Active-phase working path: the tmpdir clone. Before setup /
        // after teardown the tmpdir is gone; fall back to the base for
        // diagnostic convenience (the [`Runner`] only consults `path()`
        // while active and switches to [`final_path`] afterwards).
        match self.mirror.as_ref() {
            Some(m) => m.path(),
            None => &self.base,
        }
    }

    fn final_path(&self) -> &Path {
        // Post-teardown: apply-back has reconciled the tmpdir into the
        // base, so the base is the stable location post-teardown event
        // handlers should see.
        &self.base
    }
}

#[cfg(test)]
mod tests {
    use super::super::policy::NetworkAccess;
    use super::*;
    use tempfile::TempDir;

    fn clone_settings() -> CloneSettings {
        CloneSettings {
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: ApplyBackMode::Sync,
            apply_back_excludes: Vec::new(),
            apply_back_includes: Vec::new(),
        }
    }

    fn default_deny_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkAccess::Off,
            allow_read_outside: Vec::new(),
            allow_write_outside: Vec::new(),
            extra_deny_paths: Vec::new(),
            allow_exec: Vec::new(),
        }
    }

    #[tokio::test]
    async fn path_returns_base_before_setup() {
        let base = TempDir::new().expect("tempdir");
        let ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxRequirements::default(),
        );
        assert_eq!(ws.path(), base.path());
    }

    #[tokio::test]
    async fn teardown_without_setup_is_noop() {
        let base = TempDir::new().expect("tempdir");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxRequirements::default(),
        );
        ws.teardown(CancellationToken::new())
            .await
            .expect("noop ok");
    }

    #[tokio::test]
    async fn setup_missing_base_errors() {
        let mut ws = SandboxWorkspace::new(
            "/definitely/not/a/path/sandbox",
            clone_settings(),
            default_deny_policy(),
            SandboxRequirements::default(),
        );
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should err");
        assert!(matches!(err, SandboxWorkspaceError::NotFound(_)));
    }

    #[test]
    fn encode_decode_prefix_roundtrip() {
        use crate::decode_prefix_env;
        let prefix = vec![
            OsString::from("sandbox-exec"),
            OsString::from("-f"),
            OsString::from("/tmp/profile.sb"),
        ];
        let encoded = encode_prefix_env(&prefix);
        let decoded = decode_prefix_env(encoded.to_str().unwrap());
        assert_eq!(decoded, prefix);
    }

    #[test]
    fn decode_empty_returns_empty() {
        assert!(crate::decode_prefix_env("").is_empty());
    }
}
