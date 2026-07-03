//! [`CloneWorkspace`] — [`Workspace`] implementation that mirrors the
//! base directory into a temp tree. See the [module docs](super) for the
//! conceptual model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Workspace;
use crate::time::{Clock, SystemClock};
use crate::workspace::WorkspaceError;
use crate::workspace::workspace::{ActiveWorkspace, StdioMode, finish_spawn};
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
/// let active = ws.setup(CancellationToken::new()).await?;
/// // ... run the agent against active.path() ...
/// let persistent = active.teardown(CancellationToken::new()).await?;
/// # let _ = persistent;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct CloneWorkspace {
    base: PathBuf,
    settings: CloneSettings,
    clock: Arc<dyn Clock>,
}

impl CloneWorkspace {
    /// Create a new [`CloneWorkspace`] rooted at `base` with the given
    /// [`CloneSettings`].
    ///
    /// Every knob is supplied by the caller; iter ships no defaults.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>, settings: CloneSettings) -> Self {
        let base = base.into();
        settings.warn_if_merge_gate_defeated("clone", &base);
        Self {
            base,
            settings,
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
        let base = base.into();
        settings.warn_if_merge_gate_defeated("clone", &base);
        Self {
            base,
            settings,
            clock,
        }
    }

    /// Current apply-back mode.
    #[must_use]
    pub fn apply_back_mode(&self) -> ApplyBackMode {
        self.settings.apply_back
    }

    /// Materialise the mirror, returning the concrete [`CloneWorkspaceError`].
    /// The [`Workspace`] trait impl erases this into [`WorkspaceError`].
    ///
    /// Self-cleaning on failure: the mirror's backing `TempDir` is dropped
    /// (and therefore removed) on every error path, so a failed setup leaves
    /// nothing behind.
    ///
    /// # Errors
    ///
    /// Returns [`CloneWorkspaceError`] when the base path is missing or not a
    /// directory, when a clone/apply-back filter fails to compile, or when
    /// materialising the mirror fails.
    pub async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<ActiveCloneWorkspace, CloneWorkspaceError> {
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
        Ok(ActiveCloneWorkspace {
            base: self.base.clone(),
            mirror,
            apply_back: self.settings.apply_back,
        })
    }
}

#[async_trait]
impl Workspace for CloneWorkspace {
    async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Box<dyn ActiveWorkspace>, WorkspaceError> {
        CloneWorkspace::setup(self, cancel)
            .await
            .map(|active| Box::new(active) as Box<dyn ActiveWorkspace>)
            .map_err(WorkspaceError::new)
    }

    fn name(&self) -> &'static str {
        "clone"
    }
}

/// The active form of a [`CloneWorkspace`]: the materialised temp mirror.
///
/// Owns the [`Mirror`] outright — there is no "not yet cloned" state to
/// represent. [`teardown`](Self::teardown) reconciles the mirror back into
/// the base directory per the configured [`ApplyBackMode`] and returns the
/// base as the persistent path.
#[derive(Debug)]
pub struct ActiveCloneWorkspace {
    base: PathBuf,
    mirror: Mirror,
    apply_back: ApplyBackMode,
}

impl ActiveCloneWorkspace {
    /// Reconcile and tear down the mirror, returning the concrete
    /// [`CloneWorkspaceError`] (the trait impl erases it).
    ///
    /// # Errors
    ///
    /// Returns [`CloneWorkspaceError`] when reconciling the mirror back into
    /// the base directory (apply-back) fails. The temp tree is removed on
    /// every path — on apply-back failure the mirror is dropped and its
    /// backing `TempDir` cleans up.
    pub async fn teardown(self, cancel: CancellationToken) -> Result<PathBuf, CloneWorkspaceError> {
        // Apply-back is not interrupted mid-flight: the agent's in-flight
        // work outranks shutdown book-keeping.
        drop(cancel);
        let apply_back_result = match self.apply_back {
            ApplyBackMode::Discard => Ok(()),
            ApplyBackMode::Sync => self.mirror.sync_back().await,
            ApplyBackMode::Merge => self.mirror.merge_back().await,
        };
        // Close the mirror on every path — an implicit `Drop` on the error
        // path would run the temp tree's removal synchronously on the
        // reactor thread, while `close_best_effort` routes it through
        // `spawn_blocking`.
        self.mirror.close_best_effort().await;
        apply_back_result?;
        tracing::debug!(base = %self.base.display(), "clone workspace torn down");
        Ok(self.base)
    }
}

#[async_trait]
impl ActiveWorkspace for ActiveCloneWorkspace {
    fn path(&self) -> &Path {
        self.mirror.path()
    }

    fn spawn(
        &self,
        mut command: tokio::process::Command,
        io: StdioMode,
    ) -> std::io::Result<tokio::process::Child> {
        command.current_dir(self.mirror.path());
        finish_spawn(command, io)
    }

    async fn teardown(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<PathBuf, WorkspaceError> {
        (*self).teardown(cancel).await.map_err(WorkspaceError::new)
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
        assert!(!temp.join("node_modules").exists());
        assert!(!temp.join("vendor/foo/node_modules").exists());
        assert!(temp.join("vendor/foo").exists());
        assert!(temp.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn bare_dir_exclude_prunes_subtree_despite_child_negation() {
        // Pins the intended directory-granular clone pruning (see the
        // filter.rs module docs): a *bare-directory* exclude prunes the whole
        // subtree — the walk skips `vendor/` and never descends, so a child
        // negation cannot rescue anything within it. This is symmetric with
        // the apply-back workspace gate (filter.rs
        // `workspace_gate_masks_child_of_excluded_dir_despite_include`); the
        // documented idiom for keeping one child is exercised by
        // `contents_glob_exclude_rescues_named_child` below.
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("vendor/keep/sub.txt"), b"keep").await;
        write(&base.path().join("vendor/other/o.txt"), b"other").await;
        write(&base.path().join("src/main.rs"), b"rs").await;

        let mut s = settings();
        s.excludes = vec!["vendor".to_string(), "!vendor/keep".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
        assert!(
            !temp.join("vendor").exists(),
            "a bare-directory exclude prunes the whole subtree; a child negation is moot",
        );
        assert!(temp.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn contents_glob_exclude_rescues_named_child() {
        // The documented idiom for "drop a directory's contents but keep one
        // child": exclude at *contents* granularity so the walk still descends
        // the directory, then negate the child. Unlike a bare-directory
        // exclude, this leaves `vendor/keep` materialised while dropping its
        // siblings.
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("vendor/keep/sub.txt"), b"keep").await;
        write(&base.path().join("vendor/other/o.txt"), b"other").await;
        write(&base.path().join("src/main.rs"), b"rs").await;

        let mut s = settings();
        s.excludes = vec!["vendor/*".to_string(), "!vendor/keep".to_string()];
        let mut ws = CloneWorkspace::new(base.path(), s);
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
        assert!(
            temp.join("vendor/keep/sub.txt").exists(),
            "a contents-granularity exclude lets the negation rescue the named child",
        );
        assert!(
            !temp.join("vendor/other").exists(),
            "siblings the contents exclude matched are still dropped",
        );
        assert!(temp.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn sync_mode_copies_modifications_back() {
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"original").await;

        let mut ws = CloneWorkspace::new(base.path(), settings());
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();

        fs::write(temp.join("a.txt"), b"modified").await.expect("w");
        write(&temp.join("new.txt"), b"brand new").await;

        let persistent = active
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert_eq!(persistent, base.path());

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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();

        fs::remove_file(temp.join("deleteme.txt"))
            .await
            .expect("rm");

        active
            .teardown(CancellationToken::new())
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        let temp = active.path().to_path_buf();
        assert!(!temp.join(".git").exists(), "clone must exclude .git");
        fs::write(temp.join("src/main.rs"), b"fn main() { run(); }")
            .await
            .expect("write");

        active
            .teardown(CancellationToken::new())
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();
        fs::write(temp.join("a.txt"), b"modified").await.expect("w");
        write(&temp.join("new.txt"), b"new").await;
        active
            .teardown(CancellationToken::new())
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();

        // Delete a file in temp and modify another; wait a beat so the
        // Merge mtime check recognises the update.
        sleep(Duration::from_millis(20)).await;
        fs::remove_file(temp.join("survive.txt")).await.expect("rm");
        fs::write(temp.join("keep.txt"), b"updated")
            .await
            .expect("w");

        active
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");

        assert!(base.path().join("survive.txt").exists());
        let got = fs::read_to_string(base.path().join("keep.txt"))
            .await
            .expect("read");
        assert_eq!(got, "updated");
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();
        assert!(temp.exists());
        active
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert!(!temp.exists(), "temp dir must be removed after teardown");
    }

    #[tokio::test]
    async fn repeated_setup_teardown_cycles_on_one_workspace() {
        // The runner holds one Workspace for the whole exploration; two full
        // cycles on the same instance are the regression net for that model.
        let base = TempDir::new().expect("tempdir");
        write(&base.path().join("a.txt"), b"v0").await;
        let mut ws = CloneWorkspace::new(base.path(), settings());

        for round in 1..=2 {
            let active = ws.setup(CancellationToken::new()).await.expect("setup");
            let temp = active.path().to_path_buf();
            fs::write(temp.join("a.txt"), format!("v{round}"))
                .await
                .expect("write");
            let persistent = active
                .teardown(CancellationToken::new())
                .await
                .expect("teardown");
            assert_eq!(persistent, base.path());
            assert!(!temp.exists());
            let back = fs::read_to_string(base.path().join("a.txt"))
                .await
                .expect("read");
            assert_eq!(back, format!("v{round}"));
        }
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();

        // Agent sees the existing .md inside the temp tree.
        assert!(temp.join("README.md").exists());

        // Agent writes a new .md and also touches a non-.md file.
        write(&temp.join("HANDOFF.md"), b"agent wrote").await;
        fs::write(temp.join("src/main.rs"), b"new rs")
            .await
            .expect("w");

        active
            .teardown(CancellationToken::new())
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp_a = active.path().join("a.txt");
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
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp_a = active.path().join("a.txt");
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
