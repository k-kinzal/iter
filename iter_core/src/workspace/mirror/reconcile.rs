//! Apply-back reconciliation primitives used by [`Mirror`](super::Mirror).
//!
//! Two strategies live here:
//!
//! * [`sync_back_impl`] — rsync-style: every file present in the temp tree
//!   is copied back to the base tree, and files present in the base tree
//!   but absent from the temp tree are deleted. Empty directories left
//!   behind are pruned.
//! * [`merge_back_impl`] — conservative: files are copied back only when
//!   the temp copy is strictly newer than the base copy by mtime, and
//!   nothing is ever deleted. Useful when the caller intends to review or
//!   further process the result and does not want an accidental
//!   rm-in-temp to delete files in the base.
//!
//! Both strategies share the same [`ApplyBackFilter`]. Callers union the
//! workspace-level (clone-time) excludes into the apply-back filter at
//! construction time so that files never copied into the sandbox cannot
//! become deletion candidates during sync-back.
//!
//! # Merge mode: why mtime, not content
//!
//! An earlier implementation split the behaviour between `CloneWorkspace`
//! (mtime comparison) and `SandboxWorkspace` (byte-level comparison). The
//! split had no documented rationale and no test pinning it. The two
//! supported sandbox backends (macOS `sandbox-exec`, Linux `bwrap`) are
//! both bind-mount based, so host mtimes are authoritative inside the
//! sandbox; there is no environment in which byte comparison would be
//! strictly safer. Merge is unified on mtime here for consistency, O(1)
//! per-file cost, and a single code path to test.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::enumerate::list_files_relative;
use super::filter::ApplyBackFilter;
use super::materialize::copy_file_preserving_parents;
use super::mtime::mtime;
use super::prune::prune_empty_dirs;

/// Rsync-style reconciliation: copy changed/new files temp → base and
/// delete files in base that no longer exist in temp.
pub(crate) async fn sync_back_impl(
    base: &Path,
    temp: &Path,
    filter: &ApplyBackFilter,
) -> io::Result<()> {
    let temp_files = list_files_relative(temp, filter).await?;
    let base_files = list_files_relative(base, filter).await?;

    for rel in &temp_files {
        let src = temp.join(rel);
        let dst = base.join(rel);
        copy_file_preserving_parents(&src, &dst).await?;
    }

    let temp_set: HashSet<&PathBuf> = temp_files.iter().collect();
    for rel in &base_files {
        if temp_set.contains(rel) {
            continue;
        }
        let victim = base.join(rel);
        match fs::remove_file(&victim).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }

    prune_empty_dirs(base, filter).await?;
    Ok(())
}

/// Conservative merge: copy new/modified files temp → base without
/// deletion. A file is considered modified iff the temp copy's mtime is
/// strictly newer than the base copy's mtime.
pub(crate) async fn merge_back_impl(
    base: &Path,
    temp: &Path,
    filter: &ApplyBackFilter,
) -> io::Result<()> {
    let temp_files = list_files_relative(temp, filter).await?;
    for rel in &temp_files {
        let src = temp.join(rel);
        let dst = base.join(rel);
        if fs::try_exists(&dst).await? {
            let src_mtime = mtime(&src).await?;
            let dst_mtime = mtime(&dst).await?;
            if src_mtime <= dst_mtime {
                continue;
            }
        }
        copy_file_preserving_parents(&src, &dst).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;

    use super::super::mtime::set_file_mtime;
    use super::*;

    async fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, contents).await.expect("write");
    }

    /// Pins the "merge mode uses mtime comparison, not byte comparison"
    /// invariant. The sandbox workspace previously reached into this path
    /// via a byte-level comparison; unifying on mtime must not silently
    /// regress into either "always write" or "never write".
    #[tokio::test]
    async fn merge_back_skips_when_src_is_not_newer() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("keep.txt"), b"BASE_NEWER").await;
        write(&temp.path().join("keep.txt"), b"temp stale").await;

        let far_past = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let near_present = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        set_file_mtime(&temp.path().join("keep.txt"), far_past)
            .await
            .expect("stamp temp");
        set_file_mtime(&base.path().join("keep.txt"), near_present)
            .await
            .expect("stamp base");

        merge_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("merge ok");

        let after = fs::read_to_string(base.path().join("keep.txt"))
            .await
            .expect("read");
        assert_eq!(
            after, "BASE_NEWER",
            "base must win when its mtime is newer — merge is mtime-based, \
             not content-based",
        );
    }

    #[tokio::test]
    async fn merge_back_copies_when_src_is_newer() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("keep.txt"), b"base old").await;
        write(&temp.path().join("keep.txt"), b"TEMP_NEW").await;

        let far_past = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let near_present = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        set_file_mtime(&base.path().join("keep.txt"), far_past)
            .await
            .expect("stamp base");
        set_file_mtime(&temp.path().join("keep.txt"), near_present)
            .await
            .expect("stamp temp");

        merge_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("merge ok");

        let after = fs::read_to_string(base.path().join("keep.txt"))
            .await
            .expect("read");
        assert_eq!(after, "TEMP_NEW");
    }

    #[tokio::test]
    async fn merge_back_never_deletes() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("survive.txt"), b"stay").await;

        merge_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("merge ok");

        assert!(
            base.path().join("survive.txt").exists(),
            "merge must never delete files that the temp side does not know about",
        );
    }

    #[tokio::test]
    async fn sync_back_removes_files_missing_in_temp() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("keep.txt"), b"k").await;
        write(&base.path().join("drop.txt"), b"d").await;
        write(&temp.path().join("keep.txt"), b"k").await;

        sync_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("sync ok");

        assert!(base.path().join("keep.txt").exists());
        assert!(!base.path().join("drop.txt").exists());
    }

    /// Files excluded at workspace (clone-time) level must survive sync-back
    /// even when they are NOT listed in the `apply_back` excludes. The caller
    /// unions workspace excludes into the `ApplyBackFilter` before passing it
    /// here; this test verifies the filter-level behaviour.
    #[tokio::test]
    async fn workspace_excluded_paths_not_deleted_during_sync() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join(".git/HEAD"), b"ref: refs/heads/main\n").await;
        write(&base.path().join(".git/config"), b"[core]\n").await;
        write(&base.path().join("src/main.rs"), b"fn main() {}").await;
        write(&temp.path().join("src/main.rs"), b"fn main() { run(); }").await;

        let filter =
            ApplyBackFilter::compile(&["*.md".to_owned(), ".git".to_owned()], &[]).expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        assert!(base.path().join(".git/HEAD").exists());
        assert!(base.path().join(".git/config").exists());
        let head = fs::read_to_string(base.path().join(".git/HEAD"))
            .await
            .expect("read");
        assert_eq!(head, "ref: refs/heads/main\n");

        let main = fs::read_to_string(base.path().join("src/main.rs"))
            .await
            .expect("read");
        assert_eq!(main, "fn main() { run(); }");
    }

    /// Workspace excludes are enforced even when `apply_back_includes` is
    /// set (whitelist mode). The unconditional workspace-exclude layer must
    /// fire before the includes check.
    #[tokio::test]
    async fn workspace_excludes_override_apply_back_includes() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join(".git/HEAD"), b"ref: refs/heads/main\n").await;
        write(&base.path().join("src/main.rs"), b"fn main() {}").await;
        write(&temp.path().join("src/main.rs"), b"fn main() { run(); }").await;

        let filter = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &["**".to_owned()],
            &[".git".to_owned()],
        )
        .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        assert!(
            base.path().join(".git/HEAD").exists(),
            "workspace excludes must override includes whitelist",
        );
        let main = fs::read_to_string(base.path().join("src/main.rs"))
            .await
            .expect("read");
        assert_eq!(main, "fn main() { run(); }");
    }

    /// Workspace excludes cannot be unmasked by negation patterns in the
    /// user's `apply_back_excludes`.
    #[tokio::test]
    async fn workspace_excludes_resist_negation() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join(".git/HEAD"), b"ref: refs/heads/main\n").await;
        write(&base.path().join("src/main.rs"), b"fn main() {}").await;
        write(&temp.path().join("src/main.rs"), b"fn main() { run(); }").await;

        let filter = ApplyBackFilter::compile_with_workspace_excludes(
            &["!.git".to_owned()],
            &[],
            &[".git".to_owned()],
        )
        .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        assert!(
            base.path().join(".git/HEAD").exists(),
            "negation in apply_back_excludes must not unmask workspace excludes",
        );
    }

    /// Apply-back excludes mask files on both sides of the diff: the file
    /// is neither copied from temp nor deleted from base. This is the
    /// asymmetric-filter contract that the redesign exists to enable.
    #[tokio::test]
    async fn apply_back_excludes_skip_file_both_directions() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("HANDOFF.md"), b"old").await;
        write(&temp.path().join("HANDOFF.md"), b"NEW").await;
        write(&temp.path().join("agent_wrote.md"), b"x").await;
        write(&temp.path().join("kept.txt"), b"k").await;

        let filter = ApplyBackFilter::compile(&["*.md".to_owned()], &[]).expect("filter compiles");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        // Pre-existing .md is untouched on base — apply-back didn't copy
        // or delete it because it was filtered out of both walks.
        let kept = fs::read_to_string(base.path().join("HANDOFF.md"))
            .await
            .expect("read");
        assert_eq!(kept, "old");

        // Agent-authored .md never reached base.
        assert!(!base.path().join("agent_wrote.md").exists());

        // Non-.md changes still propagate.
        let kept_txt = fs::read_to_string(base.path().join("kept.txt"))
            .await
            .expect("read");
        assert_eq!(kept_txt, "k");
    }
}
