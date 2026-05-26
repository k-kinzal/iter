//! [`LocalWorkspace`] — [`Workspace`] implementation pointed at an
//! existing on-disk directory. See the [module docs](super) for the
//! role it plays relative to [`CloneWorkspace`](crate::workspace::CloneWorkspace)
//! and [`SandboxWorkspace`](crate::workspace::SandboxWorkspace).

use std::path::{Path, PathBuf};

use crate::Workspace;
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
/// ws.setup(CancellationToken::new()).await?;
/// assert_eq!(ws.path(), std::path::Path::new("/tmp/my-project"));
/// ws.teardown(CancellationToken::new()).await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct LocalWorkspace {
    base: PathBuf,
    set_up: bool,
}

impl LocalWorkspace {
    /// Create a new [`LocalWorkspace`] rooted at `base`.
    ///
    /// No filesystem access occurs in the constructor; the path is only
    /// checked when [`setup`](Workspace::setup) is called.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self {
            base: base.into(),
            set_up: false,
        }
    }

    /// Returns `true` if the workspace has been successfully set up.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.set_up
    }
}

impl Workspace for LocalWorkspace {
    type Error = LocalWorkspaceError;

    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), Self::Error> {
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
        self.set_up = true;
        tracing::debug!(path = %self.base.display(), "local workspace set up");
        Ok(())
    }

    async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), Self::Error> {
        // The target directory is the source of truth; there is nothing to
        // clean up. We only flip the set_up flag so that
        // [`is_set_up`] accurately reflects reality. Pure noop — nothing
        // to cancel.
        drop(cancel);
        self.set_up = false;
        tracing::debug!(path = %self.base.display(), "local workspace torn down");
        Ok(())
    }

    fn path(&self) -> &Path {
        &self.base
    }

    fn final_path(&self) -> &Path {
        self.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn setup_on_valid_dir_succeeds() {
        let dir = TempDir::new().expect("tempdir");
        let mut ws = LocalWorkspace::new(dir.path());
        ws.setup(CancellationToken::new()).await.expect("setup ok");
        assert!(ws.is_set_up());
        assert_eq!(ws.path(), dir.path());
    }

    #[tokio::test]
    async fn setup_on_missing_dir_errors() {
        let mut ws = LocalWorkspace::new("/definitely/not/a/real/path/iter_workspace_test");
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, LocalWorkspaceError::NotFound(_)));
        assert!(!ws.is_set_up());
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
    async fn teardown_is_noop() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("marker"), b"keep me")
            .await
            .expect("write");
        let mut ws = LocalWorkspace::new(dir.path());
        ws.setup(CancellationToken::new()).await.expect("setup");
        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert!(!ws.is_set_up());
        assert!(
            dir.path().join("marker").exists(),
            "teardown must not delete"
        );
    }

    #[tokio::test]
    async fn path_returns_configured_path_even_without_setup() {
        let ws = LocalWorkspace::new("/some/where");
        assert_eq!(ws.path(), Path::new("/some/where"));
    }
}
