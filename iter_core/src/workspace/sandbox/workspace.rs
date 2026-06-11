//! [`SandboxWorkspace`] — [`Workspace`] implementation that clones into
//! a tmpdir and wraps agent commands in a kernel-level sandbox.
//!
//! See the [module docs](super) for the conceptual model — the
//! "clone + wrap" pipeline, the upper/lower bound contract, and the
//! platform support matrix.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::workspace::WorkspaceError;
use crate::{SandboxRequirements, Workspace};
use async_trait::async_trait;
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
    /// project's upper-bound rule set from the declaration. `requirements`
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

    /// Returns `true` if a sandbox backend is available for the host
    /// platform and its driver binary is present on `PATH`.
    ///
    /// Intended for tests and friendly CLI diagnostics — use it to skip
    /// when the host can't enforce a sandbox.
    #[must_use]
    pub fn detect_backend_available() -> bool {
        detect_backend_available()
    }

    /// Materialise the sandbox, returning the concrete
    /// [`SandboxWorkspaceError`]. The [`Workspace`] trait impl erases this into
    /// [`WorkspaceError`].
    ///
    /// # Errors
    ///
    /// Returns [`SandboxWorkspaceError`] when cancelled, when the base path is
    /// missing or not a directory, when a filter fails to compile, when
    /// materialising the mirror fails, or when the sandbox backend cannot be
    /// prepared.
    pub async fn setup(&mut self, cancel: CancellationToken) -> Result<(), SandboxWorkspaceError> {
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

        // ----- Phase 3: retain the prefix as typed invocation data -------
        // The backend's argv prefix is command-construction data, not host
        // process state. It is stored on the workspace value and surfaced
        // through `Workspace::sandbox_command_prefix`; the runner reads it
        // after setup and threads it into the agent invocation. Nothing is
        // written to the process environment.
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

    /// Tear down the sandbox, returning the concrete [`SandboxWorkspaceError`]
    /// (see [`setup`](Self::setup) for the erasure note).
    ///
    /// # Errors
    ///
    /// Returns [`SandboxWorkspaceError`] when reconciling the mirror back into
    /// the base directory (apply-back) fails.
    pub async fn teardown(
        &mut self,
        _cancel: CancellationToken,
    ) -> Result<(), SandboxWorkspaceError> {
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

        if let Some(mut backend) = self.backend.take() {
            if let Err(e) = backend.cleanup() {
                tracing::warn!(error = %e, "sandbox backend cleanup failed; continuing teardown");
            }
        }

        if let Some(mirror) = self.mirror.take() {
            mirror.close_best_effort().await;
        }
        self.command_prefix.clear();
        self.set_up = false;
        tracing::debug!(base = %self.base.display(), "sandbox workspace torn down");
        Ok(())
    }
}

#[async_trait]
impl Workspace for SandboxWorkspace {
    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError> {
        SandboxWorkspace::setup(self, cancel)
            .await
            .map_err(WorkspaceError::new)
    }

    async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError> {
        SandboxWorkspace::teardown(self, cancel)
            .await
            .map_err(WorkspaceError::new)
    }

    fn name(&self) -> &'static str {
        "sandbox"
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

    fn sandbox_command_prefix(&self) -> &[OsString] {
        // The backend-produced wrap, retained on the value during setup.
        // Empty before setup / after teardown; the runner only reads it
        // while the workspace is active.
        &self.command_prefix
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

    #[tokio::test]
    async fn temp_dir_cleaned_up_after_teardown() {
        if !SandboxWorkspace::detect_backend_available() {
            eprintln!("skipping: no sandbox backend available");
            return;
        }
        let base = TempDir::new().expect("tempdir");
        fs::write(base.path().join("a.txt"), b"hi")
            .await
            .expect("write");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxRequirements::default(),
        );
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();
        assert!(temp.exists());
        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert!(!temp.exists(), "temp dir must be removed after teardown");
    }

    #[tokio::test]
    async fn temp_dir_cleaned_up_when_backend_cleanup_fails() {
        use crate::workspace::sandbox::backend::{BackendError, SandboxBackend, SandboxDescriptor};

        #[derive(Debug)]
        struct FailingBackend;
        impl SandboxBackend for FailingBackend {
            fn prepare(
                &mut self,
                _: &SandboxDescriptor<'_>,
            ) -> Result<Vec<OsString>, BackendError> {
                Ok(Vec::new())
            }
            fn cleanup(&mut self) -> Result<(), BackendError> {
                Err(BackendError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated cleanup failure",
                )))
            }
            fn name(&self) -> &'static str {
                "failing-test"
            }
        }

        let base = TempDir::new().expect("tempdir");
        fs::write(base.path().join("a.txt"), b"hi")
            .await
            .expect("write");

        let clone_filter = CloneFilter::compile(&[], &[]).expect("filter");
        let apply_back_filter = clone_settings().apply_back_filter().expect("abf");
        let mirror = Mirror::materialize(
            base.path().to_path_buf(),
            &clone_filter,
            apply_back_filter,
            true,
        )
        .await
        .expect("materialize");
        let temp = mirror.path().to_path_buf();

        let mut ws = SandboxWorkspace {
            base: base.path().to_path_buf(),
            settings: clone_settings(),
            policy: default_deny_policy(),
            requirements: SandboxRequirements::default(),
            mirror: Some(mirror),
            backend: Some(Box::new(FailingBackend)),
            command_prefix: Vec::new(),
            set_up: true,
        };

        assert!(temp.exists());
        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown must succeed despite backend failure");
        assert!(
            !temp.exists(),
            "temp dir must be removed even when backend cleanup fails"
        );
    }

    #[tokio::test]
    async fn sandbox_command_prefix_empty_before_setup() {
        // Before setup the workspace has produced no wrap; the trait method
        // must report an empty prefix (the same answer local/clone give),
        // never reach for ambient process state.
        let base = TempDir::new().expect("tempdir");
        let ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxRequirements::default(),
        );
        assert!(Workspace::sandbox_command_prefix(&ws).is_empty());
    }

    #[test]
    fn sandbox_command_prefix_returns_retained_wrap() {
        // The trait method surfaces exactly the argv the backend produced,
        // as typed data carried on the value — no encode/decode round-trip
        // and no environment variable in the path.
        let base = TempDir::new().expect("tempdir");
        let prefix = vec![
            OsString::from("sandbox-exec"),
            OsString::from("-f"),
            OsString::from("/tmp/profile.sb"),
        ];
        let ws = SandboxWorkspace {
            base: base.path().to_path_buf(),
            settings: clone_settings(),
            policy: default_deny_policy(),
            requirements: SandboxRequirements::default(),
            mirror: None,
            backend: None,
            command_prefix: prefix.clone(),
            set_up: true,
        };
        assert_eq!(Workspace::sandbox_command_prefix(&ws), prefix.as_slice());
    }
}
