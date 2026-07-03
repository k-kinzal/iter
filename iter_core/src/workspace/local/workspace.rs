//! [`LocalWorkspace`] — [`Workspace`] implementation pointed at an
//! existing on-disk directory. See the [module docs](super) for the
//! role it plays relative to [`CloneWorkspace`](crate::workspace::CloneWorkspace)
//! and [`SandboxWorkspace`](crate::workspace::SandboxWorkspace).

use std::path::{Path, PathBuf};

use crate::Workspace;
use crate::workspace::WorkspaceError;
use crate::workspace::workspace::{ActiveWorkspace, StdioMode, finish_spawn};
use async_trait::async_trait;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use super::LocalWorkspaceError;

/// Workspace that points at an existing, on-disk directory.
///
/// The directory is used as-is; no copy is made and no sandbox is set up.
/// This gives the agent the widest possible exploration scope because it can
/// see and modify anything inside the directory — caches, build artefacts,
/// and any other project-side state.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::Workspace;
/// use iter_core::workspace::LocalWorkspace;
/// use tokio_util::sync::CancellationToken;
///
/// let mut ws = LocalWorkspace::new("/tmp/my-project");
/// let active = Workspace::setup(&mut ws, CancellationToken::new()).await?;
/// assert_eq!(active.path(), std::path::Path::new("/tmp/my-project"));
/// let persistent = active.teardown(CancellationToken::new()).await?;
/// assert_eq!(persistent, std::path::PathBuf::from("/tmp/my-project"));
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct LocalWorkspace {
    base: PathBuf,
}

impl LocalWorkspace {
    /// Create a new [`LocalWorkspace`] rooted at `base`.
    ///
    /// No filesystem access occurs in the constructor; the path is only
    /// checked when [`setup`](Workspace::setup) is called.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Materialise the workspace, returning the concrete
    /// [`LocalWorkspaceError`]. The [`Workspace`] trait impl erases this into
    /// [`WorkspaceError`]; callers holding a concrete `LocalWorkspace` get the
    /// precise error here.
    ///
    /// Nothing is acquired on the failure path, so the self-cleaning setup
    /// contract holds trivially.
    ///
    /// # Errors
    ///
    /// Returns [`LocalWorkspaceError`] when the base path is missing or is not
    /// a directory.
    pub async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<ActiveLocalWorkspace, LocalWorkspaceError> {
        // LocalWorkspace setup is a quick validate-only step with no
        // natural cancel point; accept the token and drop it.
        drop(cancel);
        let meta = match fs::metadata(&self.base).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(LocalWorkspaceError::NotFound(self.base.clone()));
            }
            Err(e) => return Err(LocalWorkspaceError::Io(e)),
        };
        if !meta.is_dir() {
            return Err(LocalWorkspaceError::NotADirectory(self.base.clone()));
        }
        tracing::debug!(path = %self.base.display(), "local workspace set up");
        Ok(ActiveLocalWorkspace {
            path: self.base.clone(),
        })
    }
}

#[async_trait]
impl Workspace for LocalWorkspace {
    async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Box<dyn ActiveWorkspace>, WorkspaceError> {
        LocalWorkspace::setup(self, cancel)
            .await
            .map(|active| Box::new(active) as Box<dyn ActiveWorkspace>)
            .map_err(WorkspaceError::new)
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

/// The active form of a [`LocalWorkspace`]: the validated base directory
/// itself. The working path *is* the persistent path, so teardown has
/// nothing to reconcile.
#[derive(Debug)]
pub struct ActiveLocalWorkspace {
    path: PathBuf,
}

#[async_trait]
impl ActiveWorkspace for ActiveLocalWorkspace {
    fn path(&self) -> &Path {
        &self.path
    }

    fn spawn(
        &self,
        mut command: tokio::process::Command,
        io: StdioMode,
    ) -> std::io::Result<tokio::process::Child> {
        command.current_dir(&self.path);
        finish_spawn(command, io)
    }

    async fn teardown(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<PathBuf, WorkspaceError> {
        // The target directory is the source of truth; there is nothing to
        // clean up. Pure noop — nothing to cancel.
        drop(cancel);
        tracing::debug!(path = %self.path.display(), "local workspace torn down");
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn setup_on_valid_dir_yields_active_at_base() {
        let dir = TempDir::new().expect("tempdir");
        let mut ws = LocalWorkspace::new(dir.path());
        let active = ws.setup(CancellationToken::new()).await.expect("setup ok");
        assert_eq!(active.path(), dir.path());
    }

    #[tokio::test]
    async fn setup_on_missing_dir_errors() {
        let mut ws = LocalWorkspace::new("/definitely/not/a/real/path/iter_workspace_test");
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, LocalWorkspaceError::NotFound(_)));
    }

    #[tokio::test]
    async fn setup_on_file_errors() {
        let dir = TempDir::new().expect("tempdir");
        let file = dir.path().join("file.txt");
        fs::write(&file, b"hi").await.expect("write");
        let mut ws = LocalWorkspace::new(&file);
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, LocalWorkspaceError::NotADirectory(_)));
    }

    #[tokio::test]
    async fn teardown_returns_base_and_deletes_nothing() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("marker"), b"keep me")
            .await
            .expect("write");
        let mut ws = LocalWorkspace::new(dir.path());
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let persistent = Box::new(active)
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert_eq!(persistent, dir.path());
        assert!(
            dir.path().join("marker").exists(),
            "teardown must not delete"
        );
    }

    #[tokio::test]
    async fn repeated_setup_teardown_cycles_on_one_workspace() {
        // The runner holds one Workspace for the whole exploration and
        // brackets every iteration with setup/teardown; two full cycles on
        // the same instance are the regression net for that model.
        let dir = TempDir::new().expect("tempdir");
        let mut ws = LocalWorkspace::new(dir.path());
        for _ in 0..2 {
            let active = ws.setup(CancellationToken::new()).await.expect("setup");
            let persistent = Box::new(active)
                .teardown(CancellationToken::new())
                .await
                .expect("teardown");
            assert_eq!(persistent, dir.path());
        }
    }
}
