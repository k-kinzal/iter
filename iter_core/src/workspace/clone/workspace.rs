//! [`CloneWorkspace`] — [`Workspace`] implementation that mirrors the
//! base directory into a temp tree. See the [module docs](super) for the
//! conceptual model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Workspace;
use crate::time::{Clock, SystemClock};
use crate::workspace::WorkspaceError;
use async_trait::async_trait;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::workspace::apply_back::ApplyBackMode;
use crate::workspace::mirror::{CloneFilter, Mirror};

use super::{CloneSettings, CloneWorkspaceError};

/// Workspace that clones a base directory into a temporary location.
///
/// See the [module docs](super) for the conceptual model and the
/// [`ApplyBackMode`] variants for reconciliation behaviour.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::Workspace;
/// use iter_core::workspace::{ApplyBackMode, CloneSettings, CloneWorkspace};
/// use tokio_util::sync::CancellationToken;
///
/// let mut ws = CloneWorkspace::new(
///     "/tmp/my-project",
///     CloneSettings {
///         excludes: Vec::new(),
///         includes: Vec::new(),
///         preserve_mtime: true,
///         apply_back: ApplyBackMode::Sync,
///         apply_back_excludes: Vec::new(),
///         apply_back_includes: Vec::new(),
///     },
/// );
/// ws.setup(CancellationToken::new()).await?;
/// // ... run the agent against ws.path() ...
/// ws.teardown(CancellationToken::new()).await?;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct CloneWorkspace {
    base: PathBuf,
    settings: CloneSettings,
    mirror: Option<Mirror>,
    set_up: bool,
    clock: Arc<dyn Clock>,
}

impl CloneWorkspace {
    /// Create a new [`CloneWorkspace`] rooted at `base` with the given
    /// [`CloneSettings`].
    ///
    /// Every knob is supplied by the caller; iter ships no defaults.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>, settings: CloneSettings) -> Self {
        Self {
            base: base.into(),
            settings,
            mirror: None,
            set_up: false,
            clock: Arc::new(SystemClock),
        }
    }

    /// Create a new [`CloneWorkspace`] with an injected clock.
    #[must_use]
    pub fn with_clock(
        base: impl Into<PathBuf>,
        settings: CloneSettings,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            base: base.into(),
            settings,
            mirror: None,
            set_up: false,
            clock,
        }
    }

    /// Returns `true` if the workspace has been successfully set up.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.set_up
    }

    /// Current apply-back mode.
    #[must_use]
    pub fn apply_back_mode(&self) -> ApplyBackMode {
        self.settings.apply_back
    }

    /// Reconcile the mirror back into `base` according to the configured
    /// mode. Split out of [`Workspace::teardown`] so the logic can be
    /// exercised directly from tests without worrying about `TempDir` drop
    /// order.
    async fn apply_back_to_base(&self) -> Result<(), CloneWorkspaceError> {
        let Some(mirror) = self.mirror.as_ref() else {
            return Err(CloneWorkspaceError::NotSetUp);
        };
        match self.settings.apply_back {
            ApplyBackMode::Discard => Ok(()),
            ApplyBackMode::Sync => Ok(mirror.sync_back().await?),
            ApplyBackMode::Merge => Ok(mirror.merge_back().await?),
        }
    }

    /// Materialise the mirror, returning the concrete [`CloneWorkspaceError`].
    /// The [`Workspace`] trait impl erases this into [`WorkspaceError`].
    ///
    /// # Errors
    ///
    /// Returns [`CloneWorkspaceError`] when the base path is missing or not a
    /// directory, when a clone/apply-back filter fails to compile, or when
    /// materialising the mirror fails.
    pub async fn setup(&mut self, cancel: CancellationToken) -> Result<(), CloneWorkspaceError> {
        // The copy path is pure filesystem work with no natural cancel point;
        // accept the token for signature compatibility and drop it.
        drop(cancel);
        let meta = match fs::metadata(&self.base).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CloneWorkspaceError::NotFound(self.base.clone()));
            }
            Err(e) => return Err(CloneWorkspaceError::Io(e)),
        };
        if !meta.is_dir() {
            return Err(CloneWorkspaceError::NotADirectory(self.base.clone()));
        }

        let clone_filter = CloneFilter::compile(&self.settings.excludes, &self.settings.includes)?;
        let apply_back_filter = self.settings.apply_back_filter()?;
        let mirror = Mirror::materialize_with_clock(
            self.base.clone(),
            &clone_filter,
            apply_back_filter,
            self.settings.preserve_mtime,
            Arc::clone(&self.clock),
        )
        .await?;

        tracing::debug!(
            base = %self.base.display(),
            temp = %mirror.path().display(),
            mode = ?self.settings.apply_back,
            "clone workspace set up",
        );
        self.mirror = Some(mirror);
        self.set_up = true;
        Ok(())
    }

    /// Reconcile and tear down the mirror, returning the concrete
    /// [`CloneWorkspaceError`] (see [`setup`](Self::setup) for the erasure
    /// note).
    ///
    /// # Errors
    ///
    /// Returns [`CloneWorkspaceError`] when reconciling the mirror back into
    /// the base directory (apply-back) fails.
    pub async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), CloneWorkspaceError> {
        drop(cancel);
        if !self.set_up {
            // Teardown without setup is a no-op rather than an error so it
            // can be safely used in Drop/bail paths.
            return Ok(());
        }
        self.apply_back_to_base().await?;

        if let Some(mirror) = self.mirror.take() {
            mirror.close_best_effort().await;
        }
        self.set_up = false;
        tracing::debug!(base = %self.base.display(), "clone workspace torn down");
        Ok(())
    }
}

#[async_trait]
impl Workspace for CloneWorkspace {
    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError> {
        CloneWorkspace::setup(self, cancel)
            .await
            .map_err(WorkspaceError::new)
    }

    async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError> {
        CloneWorkspace::teardown(self, cancel)
            .await
            .map_err(WorkspaceError::new)
    }

    fn name(&self) -> &'static str {
        "clone"
    }

    fn path(&self) -> &Path {
        // Active-phase working path: the temp clone the agent operates
        // against. Before setup / after teardown the temp dir is gone, so
        // we fall back to the base path for diagnostic convenience; the
        // [`Runner`](crate::Runner) only reads `path()` during the
        // active phase and uses [`final_path`] after teardown instead.
        match self.mirror.as_ref() {
            Some(m) => m.path(),
            None => &self.base,
        }
    }

    fn final_path(&self) -> &Path {
        // Persistent path: after teardown, apply-back has reconciled the
        // temp copy into the base directory, so `&self.base` is the
        // stable location of the agent's work.
        &self.base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Clock, SystemClock};
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;
    use tokio::time::sleep;

    fn settings() -> CloneSettings {
        CloneSettings {
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: ApplyBackMode::Sync,
            apply_back_excludes: Vec::new(),
            apply_back_includes: Vec::new(),
        }
    }

    async fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, contents).await.expect("write");
    }

    #[tokio::test]
    async fn setup_copies_entire_tree_when_excludes_empty() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("keep.txt"), b"keep").await;
        write(&base.path().join("sub/nested.txt"), b"nested").await;
        write(&base.path().join("aux/inside.txt"), b"aux").await;

        let mut ws = CloneWorkspace::new(base.path(), settings());
        ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = ws.path().to_path_buf();
        assert_ne!(temp, base.path());
        assert!(temp.join("keep.txt").exists());
        assert!(temp.join("sub/nested.txt").exists());
        assert!(temp.join("aux/inside.txt").exists());
    }

    #[tokio::test]
    async fn explicit_clone_excludes_skip_matching_paths() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("keep.txt"), b"keep").await;
        write(&base.path().join("ignore/inside.txt"), b"skip").await;

        let mut s = settings();
        s.excludes = vec!["ignore".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = ws.path().to_path_buf();
        assert!(temp.join("keep.txt").exists());
        assert!(!temp.join("ignore").exists());
    }

    #[tokio::test]
    async fn glob_clone_excludes_skip_descendants_only() {
        // Pins the new glob semantics: `docs/**/*.md` only matches under
        // `docs/`, leaving same-name files in other directories alone.
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("docs/a/b/c.md"), b"deep").await;
        write(&base.path().join("docs/top.md"), b"top").await;
        write(&base.path().join("src/foo.md"), b"src").await;
        write(&base.path().join("src/main.rs"), b"rs").await;

        let mut s = settings();
        s.excludes = vec!["docs/**/*.md".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = ws.path().to_path_buf();
        assert!(!temp.join("docs/a/b/c.md").exists());
        assert!(!temp.join("docs/top.md").exists());
        assert!(temp.join("src/foo.md").exists());
        assert!(temp.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn bare_pattern_excludes_match_basename_anywhere() {
        // `excludes = ["node_modules"]` must match both top-level
        // `./node_modules/...` and nested `./vendor/foo/node_modules/...`.
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("node_modules/a.json"), b"a").await;
        write(&base.path().join("vendor/foo/node_modules/b.json"), b"b").await;
        write(&base.path().join("src/main.rs"), b"rs").await;

        let mut s = settings();
        s.excludes = vec!["node_modules".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = ws.path().to_path_buf();
        assert!(!temp.join("node_modules").exists());
        assert!(!temp.join("vendor/foo/node_modules").exists());
        assert!(temp.join("vendor/foo").exists());
        assert!(temp.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn path_returns_base_before_setup() {
        let base = TempDir::new().expect("tempdir");
        let ws = CloneWorkspace::new(base.path(), settings());
        assert_eq!(ws.path(), base.path());
    }

    #[tokio::test]
    async fn sync_mode_copies_modifications_back() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"original").await;

        let mut ws = CloneWorkspace::new(base.path(), settings());
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();

        fs::write(temp.join("a.txt"), b"modified").await.expect("w");
        write(&temp.join("new.txt"), b"brand new").await;

        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        let back = fs::read_to_string(base.path().join("a.txt"))
            .await
            .expect("read");
        assert_eq!(back, "modified");
        let new = fs::read_to_string(base.path().join("new.txt"))
            .await
            .expect("read");
        assert_eq!(new, "brand new");
    }

    #[tokio::test]
    async fn sync_mode_deletes_removed_files() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"keep").await;
        write(&base.path().join("deleteme.txt"), b"bye").await;

        let mut ws = CloneWorkspace::new(base.path(), settings());
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();

        fs::remove_file(temp.join("deleteme.txt"))
            .await
            .expect("rm");

        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        assert!(base.path().join("a.txt").exists());
        assert!(!base.path().join("deleteme.txt").exists());
    }

    #[tokio::test]
    async fn sync_mode_preserves_workspace_excluded_paths() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join(".git/HEAD"), b"ref: refs/heads/main\n").await;
        write(&base.path().join(".git/config"), b"[core]\n").await;
        write(&base.path().join("src/main.rs"), b"fn main() {}").await;

        let mut s = settings();
        s.excludes = vec![".git".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = ws.path().to_path_buf();
        assert!(!temp.join(".git").exists(), "clone must exclude .git");
        fs::write(temp.join("src/main.rs"), b"fn main() { run(); }")
            .await
            .expect("write");

        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        assert!(
            base.path().join(".git/HEAD").exists(),
            "workspace-excluded .git must survive sync-back",
        );
        let head = fs::read_to_string(base.path().join(".git/HEAD"))
            .await
            .expect("read");
        assert_eq!(head, "ref: refs/heads/main\n");
        let main = fs::read_to_string(base.path().join("src/main.rs"))
            .await
            .expect("read");
        assert_eq!(main, "fn main() { run(); }");
    }

    #[tokio::test]
    async fn discard_mode_leaves_base_unchanged() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"original").await;

        let mut s = settings();
        s.apply_back = ApplyBackMode::Discard;
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();
        fs::write(temp.join("a.txt"), b"modified").await.expect("w");
        write(&temp.join("new.txt"), b"new").await;
        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        let back = fs::read_to_string(base.path().join("a.txt"))
            .await
            .expect("read");
        assert_eq!(back, "original");
        assert!(!base.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn merge_mode_never_deletes() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("keep.txt"), b"keep").await;
        write(&base.path().join("survive.txt"), b"hi").await;

        let mut s = settings();
        s.apply_back = ApplyBackMode::Merge;
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();

        // Delete a file in temp and modify another; wait a beat so the
        // Merge mtime check recognises the update.
        sleep(Duration::from_millis(20)).await;
        fs::remove_file(temp.join("survive.txt")).await.expect("rm");
        fs::write(temp.join("keep.txt"), b"updated")
            .await
            .expect("w");

        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        assert!(base.path().join("survive.txt").exists());
        let got = fs::read_to_string(base.path().join("keep.txt"))
            .await
            .expect("read");
        assert_eq!(got, "updated");
    }

    #[tokio::test]
    async fn teardown_without_setup_is_noop() {
        let base = TempDir::new().expect("tempdir");
        let mut ws = CloneWorkspace::new(base.path(), settings());
        ws.teardown(CancellationToken::new())
            .await
            .expect("noop ok");
    }

    #[tokio::test]
    async fn setup_missing_base_errors() {
        let mut ws = CloneWorkspace::new("/definitely/missing/clone/workspace", settings());
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should err");
        assert!(matches!(err, CloneWorkspaceError::NotFound(_)));
    }

    #[tokio::test]
    async fn temp_dir_cleaned_up_after_teardown() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"hi").await;
        let mut ws = CloneWorkspace::new(base.path(), settings());
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();
        assert!(temp.exists());
        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert!(!temp.exists(), "temp dir must be removed after teardown");
    }

    #[tokio::test]
    async fn clone_includes_override_clone_excludes() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("keep.txt"), b"keep").await;
        write(&base.path().join("hidden/value.txt"), b"ref").await;
        write(&base.path().join("drop/me.txt"), b"x").await;

        let mut s = settings();
        s.excludes = vec!["hidden".to_string(), "drop".to_string()];
        s.includes = vec!["hidden".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();
        assert!(temp.join("keep.txt").exists());
        assert!(
            temp.join("hidden/value.txt").exists(),
            "includes must rescue an otherwise-excluded path",
        );
        assert!(
            !temp.join("drop").exists(),
            "non-included excludes must still drop the path",
        );
    }

    /// The asymmetric-filter contract in action: `*.md` is **not** in the
    /// clone excludes, so the agent sees existing `.md` files in the temp
    /// tree. `*.md` **is** in the apply-back excludes, so any `.md` the
    /// agent writes never propagates back to base on `Sync` teardown.
    #[tokio::test]
    async fn apply_back_excludes_block_md_propagation() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("README.md"), b"existing").await;
        write(&base.path().join("src/main.rs"), b"rs").await;

        let mut s = settings();
        s.apply_back_excludes = vec!["*.md".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = ws.path().to_path_buf();

        // Agent sees the existing .md inside the temp tree.
        assert!(temp.join("README.md").exists());

        // Agent writes a new .md and also touches a non-.md file.
        write(&temp.join("HANDOFF.md"), b"agent wrote").await;
        fs::write(temp.join("src/main.rs"), b"new rs")
            .await
            .expect("w");

        ws.teardown(CancellationToken::new())
            .await
            .expect("teardown");

        // .md from agent did NOT leak back to base.
        assert!(
            !base.path().join("HANDOFF.md").exists(),
            "agent-written .md must be filtered out of apply-back",
        );
        // Pre-existing .md on base is untouched (apply-back never saw it).
        let readme = fs::read_to_string(base.path().join("README.md"))
            .await
            .expect("read");
        assert_eq!(readme, "existing");
        // Non-.md changes did propagate.
        let main = fs::read_to_string(base.path().join("src/main.rs"))
            .await
            .expect("read");
        assert_eq!(main, "new rs");
    }

    #[tokio::test]
    async fn preserve_mtime_true_copies_source_timestamp() {
        let base = TempDir::new().expect("tempdir");
        let src = base.path().join("a.txt");
        write(&src, b"hi").await;
        let stamped = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        crate::workspace::mirror::mtime::set_file_mtime(&src, stamped)
            .await
            .expect("stamp");

        let mut s = settings();
        s.preserve_mtime = true;
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp_a = ws.path().join("a.txt");
        let copied = fs::metadata(&temp_a).await.expect("meta");
        let copied_mtime = copied.modified().expect("mtime");
        assert_eq!(copied_mtime, stamped);
    }

    #[tokio::test]
    async fn preserve_mtime_false_stamps_clone_with_now() {
        let base = TempDir::new().expect("tempdir");
        let src = base.path().join("a.txt");
        write(&src, b"hi").await;
        let stamped = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        crate::workspace::mirror::mtime::set_file_mtime(&src, stamped)
            .await
            .expect("stamp");

        let before = SystemClock.system_time();
        let mut s = settings();
        s.preserve_mtime = false;
        let mut ws = CloneWorkspace::new(base.path(), s);
        ws.setup(CancellationToken::new()).await.expect("setup");
        let temp_a = ws.path().join("a.txt");
        let copied = fs::metadata(&temp_a).await.expect("meta");
        let copied_mtime = copied.modified().expect("mtime");
        assert!(
            copied_mtime >= before,
            "clone with preserve_mtime=false must stamp recent times \
             (got {copied_mtime:?}, expected >= {before:?})",
        );
        assert_ne!(copied_mtime, stamped);
    }
}
