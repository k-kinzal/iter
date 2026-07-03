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
use super::materialize::{copy_file_preserving_parents, remove_symlink};
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
        copy_file_preserving_parents(base, rel, &src).await?;
    }

    let temp_set: HashSet<&PathBuf> = temp_files.iter().collect();
    for rel in &base_files {
        if temp_set.contains(rel) {
            continue;
        }
        let victim = base.join(rel);
        // A base entry enumerated as a file/symlink may have been replaced by
        // a real directory during the copy phase above — this happens when a
        // stale base-side symlink shares a path with a directory the temp
        // tree now populates (see `copy_file_preserving_parents`). That
        // directory holds freshly applied temp content, not a deletion
        // candidate, so skip it. Guard by (non-following) type first:
        // `remove_file` on a directory is non-portable (EISDIR on Linux, EPERM
        // on macOS). A base-side *directory* symlink or junction the agent
        // legitimately removed is still a genuine deletion candidate, but its
        // link must be dropped with the same per-platform primitive the write
        // path uses (`remove_file`/`DeleteFileW` cannot delete a directory
        // reparse point on Windows) — route symlinks through `remove_symlink`
        // so the two removal sites can never diverge again.
        let ft = match fs::symlink_metadata(&victim).await {
            Ok(meta) => meta.file_type(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if ft.is_dir() {
            continue;
        }
        let removal = if ft.is_symlink() {
            // Drop only the link, never its (possibly out-of-base) target.
            remove_symlink(&victim, ft).await
        } else {
            fs::remove_file(&victim).await
        };
        match removal {
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
        copy_file_preserving_parents(base, rel, &src).await?;
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
            &[],
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
            &[],
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

    /// A clone-time include that rescues a *file-pattern*-excluded path pulls
    /// that file into the sandbox (the clone walk descends normally because no
    /// directory is excluded), so at teardown it must be allowed to propagate
    /// back — the workspace gate reproduces the clone walk's decision rather
    /// than masking the rescued file as the old exclude-only floor did.
    /// Meanwhile a sibling the clone filter genuinely excluded stays gated, so
    /// a base file the agent never saw is neither overwritten nor deleted.
    #[tokio::test]
    async fn workspace_gate_lets_clone_rescued_file_apply_back() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        // Clone excludes `*.secret` but rescues `keep.secret`; only the
        // rescued file (plus a normal file) was materialised into the sandbox.
        write(&base.path().join("keep.secret"), b"old").await;
        write(&base.path().join("drop.secret"), b"private").await;
        write(&base.path().join("app.txt"), b"base").await;
        write(&temp.path().join("keep.secret"), b"NEW").await;
        write(&temp.path().join("app.txt"), b"base").await;

        // Mirrors CloneSettings::apply_back_filter: workspace gate carries the
        // clone excludes/includes; no apply-back-level filters.
        let filter = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &[],
            &["*.secret".to_owned()],
            &["keep.secret".to_owned()],
        )
        .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        // The rescued file propagated back with the agent's edit.
        let keep = fs::read_to_string(base.path().join("keep.secret"))
            .await
            .expect("read");
        assert_eq!(keep, "NEW", "clone-rescued file must apply back");

        // The genuinely-excluded sibling — never in the sandbox — is left
        // exactly as it was, neither overwritten nor deleted.
        let secret = fs::read_to_string(base.path().join("drop.secret"))
            .await
            .expect("read");
        assert_eq!(secret, "private", "gated sibling must not be deleted");
    }

    /// Safety guard: when the clone filter excludes a *directory*, the walk
    /// prunes at that directory and never materialises its children — so even
    /// a clone-time include naming a child cannot pull it into the sandbox.
    /// The apply-back gate must keep the whole subtree masked: a base file that
    /// never entered the sandbox must not be deleted just because it is absent
    /// from the temp tree.
    #[tokio::test]
    async fn workspace_gate_never_deletes_child_of_excluded_dir() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        // `vendor/` is dir-excluded, so nothing under it is in temp. The agent
        // did create an unrelated file.
        write(&base.path().join("vendor/keep"), b"precious").await;
        write(&base.path().join("app.txt"), b"old").await;
        write(&temp.path().join("app.txt"), b"NEW").await;

        // Clone excludes the directory `vendor` yet names a child in includes.
        let filter = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &[],
            &["vendor".to_owned()],
            &["vendor/keep".to_owned()],
        )
        .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        // The un-cloned base file under the excluded dir survives untouched.
        assert!(
            base.path().join("vendor/keep").exists(),
            "a child of an excluded directory must never be deleted by apply-back",
        );
        let precious = fs::read_to_string(base.path().join("vendor/keep"))
            .await
            .expect("read");
        assert_eq!(precious, "precious");

        // The agent's real edit still propagated.
        let app = fs::read_to_string(base.path().join("app.txt"))
            .await
            .expect("read");
        assert_eq!(app, "NEW");
    }

    /// A `!negation` in `apply_back_excludes` that names a child of an
    /// *excluded directory* must rescue that child through the full reconcile
    /// — "apply back everything under `vendor` except drop the directory, but
    /// keep `vendor/keep`". The apply-back walk must descend into the excluded
    /// `vendor` far enough to reach the negated child; pruning at `vendor`
    /// (which the leaf filter alone would force) leaves the negation dead and
    /// the agent's edit to `vendor/keep` silently lost.
    #[tokio::test]
    async fn apply_back_negation_rescues_child_of_excluded_dir() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        // Base and temp both carry the vendor subtree; the agent edited both
        // the rescued child and an excluded sibling, plus an unrelated file.
        write(&base.path().join("vendor/keep"), b"old").await;
        write(&base.path().join("vendor/other"), b"kept-private").await;
        write(&base.path().join("app.txt"), b"old").await;
        write(&temp.path().join("vendor/keep"), b"NEW").await;
        write(&temp.path().join("vendor/other"), b"agent-scratch").await;
        write(&temp.path().join("app.txt"), b"NEW").await;

        // "exclude the vendor directory from apply-back, except vendor/keep"
        let filter =
            ApplyBackFilter::compile(&["vendor".to_owned(), "!vendor/keep".to_owned()], &[])
                .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        // The negation rescued the child despite the directory exclude: the
        // walk descended into vendor and propagated the agent's edit.
        let keep = fs::read_to_string(base.path().join("vendor/keep"))
            .await
            .expect("read");
        assert_eq!(
            keep, "NEW",
            "a negation must rescue a child of an excluded directory",
        );

        // Its excluded sibling was masked in *both* directions: neither the
        // agent's scratch edit copied over it, nor was it deleted.
        let other = fs::read_to_string(base.path().join("vendor/other"))
            .await
            .expect("read");
        assert_eq!(other, "kept-private", "excluded sibling stays untouched");

        // The unrelated file propagated normally.
        let app = fs::read_to_string(base.path().join("app.txt"))
            .await
            .expect("read");
        assert_eq!(app, "NEW");
    }

    /// Prune must reclaim a directory the delete phase just emptied, even when
    /// it sits inside an excluded subtree a *nested* negation reached into.
    /// `["vendor", "!vendor/keep/sub.txt"]` rescues `vendor/keep/sub.txt` for
    /// enumeration; when the agent removed it from temp, the delete phase drops
    /// it and `vendor/keep`/`vendor` fall empty. Because prune mirrors the
    /// enumerate/delete walk (`should_descend`, not `is_excluded`), it descends
    /// to reclaim them — an `is_excluded`-gated prune would leave stale empty
    /// dirs behind, violating this module's own contract.
    #[tokio::test]
    async fn prune_reclaims_dir_emptied_inside_negation_rescued_subtree() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");

        write(&base.path().join("vendor/keep/sub.txt"), b"old").await;
        write(&base.path().join("app.txt"), b"old").await;
        // The agent removed the vendor tree and kept app.txt.
        write(&temp.path().join("app.txt"), b"NEW").await;

        let filter = ApplyBackFilter::compile(
            &["vendor".to_owned(), "!vendor/keep/sub.txt".to_owned()],
            &[],
        )
        .expect("filter");

        sync_back_impl(base.path(), temp.path(), &filter)
            .await
            .expect("sync ok");

        assert!(
            !base.path().join("vendor/keep/sub.txt").exists(),
            "the negation-rescued file the agent removed must be deleted from base",
        );
        assert!(
            !base.path().join("vendor").exists(),
            "prune must reclaim the now-empty subtree, not leave stale empty dirs",
        );
        assert!(base.path().join("app.txt").exists());
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

    /// Apply-back writes agent-influenced content back into the *persistent*
    /// base tree, host-side, after the sandbox is gone. If the base holds a
    /// symlink at a path the temp tree now has as a regular file — a stale
    /// link reproduced by value in a prior iteration, or a benign repo
    /// symlink the agent replaced with a real file — a naive `fs::copy`
    /// follows the link and truncates its target, which may live *outside*
    /// the workspace. Apply-back must never follow a base-side symlink out of
    /// `base`; it must replace the link with a regular file in place.
    #[cfg(unix)]
    #[tokio::test]
    async fn sync_back_does_not_follow_dst_symlink_out_of_base() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");
        let outside = TempDir::new().expect("outside");

        // A sensitive host file OUTSIDE the workspace base.
        let secret = outside.path().join("secret.txt");
        write(&secret, b"TOP SECRET").await;

        // The base tree holds a symlink `link.txt` -> the outside secret.
        let link = base.path().join("link.txt");
        fs::symlink(&secret, &link).await.expect("symlink");

        // The temp tree has a *regular file* at the same relative path.
        write(&temp.path().join("link.txt"), b"attacker-controlled").await;

        sync_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("sync ok");

        let secret_after = fs::read_to_string(&secret).await.expect("read secret");
        assert_eq!(
            secret_after, "TOP SECRET",
            "apply-back must not follow a base-side symlink and write through \
             it to a target outside the workspace",
        );

        let link_meta = fs::symlink_metadata(&link).await.expect("lstat link");
        assert!(
            !link_meta.file_type().is_symlink(),
            "the stale symlink should have been replaced by a regular file",
        );
        let link_body = fs::read_to_string(&link).await.expect("read link");
        assert_eq!(link_body, "attacker-controlled");
    }

    /// The ancestor variant: a base-side symlink sits at a *directory*
    /// position that the temp tree now populates as a real directory. A
    /// naive `create_dir_all(parent)` treats the followed symlink as an
    /// already-existing directory and the subsequent file write lands in the
    /// link's target — a directory outside `base`. Apply-back must replace
    /// the symlinked ancestor with a real directory inside `base`.
    #[cfg(unix)]
    #[tokio::test]
    async fn sync_back_does_not_follow_ancestor_symlink_out_of_base() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");
        let outside = TempDir::new().expect("outside");

        // A sensitive host directory OUTSIDE the workspace base.
        let outside_dir = outside.path().join("vault");
        fs::create_dir_all(&outside_dir)
            .await
            .expect("mkdir outside");

        // The base tree holds a symlink `vault` -> the outside directory.
        let link_dir = base.path().join("vault");
        fs::symlink(&outside_dir, &link_dir)
            .await
            .expect("symlink dir");

        // The temp tree has a real directory `vault/` with a file inside.
        write(&temp.path().join("vault/loot.txt"), b"exfiltrated").await;

        sync_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("sync ok");

        assert!(
            !outside_dir.join("loot.txt").exists(),
            "apply-back must not follow a base-side symlinked ancestor and \
             write a file into a directory outside the workspace",
        );

        let dir_meta = fs::symlink_metadata(&link_dir).await.expect("lstat vault");
        assert!(
            dir_meta.file_type().is_dir(),
            "the stale symlinked ancestor should have been replaced by a real \
             directory inside base",
        );
        assert!(
            base.path().join("vault/loot.txt").exists(),
            "the applied file must land inside base, not in the link target",
        );
    }

    /// Merge mode funnels through the same write primitive, and its
    /// `try_exists` guard *follows* symlinks, so it does not block the escape
    /// on its own. When the temp file is newer than the (followed) link
    /// target, merge must still refuse to write through the link.
    #[cfg(unix)]
    #[tokio::test]
    async fn merge_back_does_not_follow_dst_symlink_out_of_base() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");
        let outside = TempDir::new().expect("outside");

        let secret = outside.path().join("secret.txt");
        write(&secret, b"TOP SECRET").await;

        let link = base.path().join("link.txt");
        fs::symlink(&secret, &link).await.expect("symlink");

        write(&temp.path().join("link.txt"), b"attacker-controlled").await;

        // Ensure the temp file is strictly newer than the (followed) target
        // so merge does not skip it for mtime reasons.
        let far_past = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let near_present = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        set_file_mtime(&secret, far_past)
            .await
            .expect("stamp secret");
        set_file_mtime(&temp.path().join("link.txt"), near_present)
            .await
            .expect("stamp temp");

        merge_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("merge ok");

        let secret_after = fs::read_to_string(&secret).await.expect("read secret");
        assert_eq!(
            secret_after, "TOP SECRET",
            "merge apply-back must not follow a base-side symlink out of base",
        );
        let link_meta = fs::symlink_metadata(&link).await.expect("lstat link");
        assert!(
            !link_meta.file_type().is_symlink(),
            "merge should have replaced the stale symlink with a regular file",
        );
    }

    /// A base-side *directory* symlink the agent removed inside the sandbox is
    /// a legitimate stale-deletion candidate: the walk never follows it, so it
    /// is enumerated as a leaf present in the base list but absent from temp.
    /// Deleting it must drop *only the link* — never its (possibly out-of-base)
    /// target — and must not spuriously fail. This pins the deletion path
    /// through `remove_symlink`, keeping it consistent with the write path's
    /// non-following removal (and, on Windows, using `remove_dir` for a
    /// directory reparse point rather than a `remove_file` that would reject
    /// it).
    #[cfg(unix)]
    #[tokio::test]
    async fn sync_back_deletes_stale_dir_symlink_without_touching_target() {
        let base = TempDir::new().expect("base");
        let temp = TempDir::new().expect("temp");
        let outside = TempDir::new().expect("outside");

        // A host directory OUTSIDE base, holding a file the link must not
        // disturb when the link itself is deleted.
        let outside_dir = outside.path().join("vault");
        write(&outside_dir.join("keep.txt"), b"UNTOUCHED").await;

        // The base tree holds a directory symlink `link` -> the outside dir,
        // plus an ordinary file so sync-back has other work to do too.
        let link = base.path().join("link");
        fs::symlink(&outside_dir, &link).await.expect("symlink dir");
        write(&base.path().join("keep.txt"), b"k").await;

        // The temp tree kept `keep.txt` but no longer has `link`: the agent
        // removed the symlinked directory inside the sandbox.
        write(&temp.path().join("keep.txt"), b"k").await;

        sync_back_impl(base.path(), temp.path(), &ApplyBackFilter::empty())
            .await
            .expect("sync ok");

        // The stale link is gone from base…
        assert!(
            fs::symlink_metadata(&link).await.is_err(),
            "the stale directory symlink should have been deleted from base",
        );
        // …but only the link: the out-of-base target and its contents survive.
        assert!(
            outside_dir.join("keep.txt").exists(),
            "deleting a base-side directory symlink must not recurse into or \
             delete its out-of-base target",
        );
        let kept = fs::read_to_string(outside_dir.join("keep.txt"))
            .await
            .expect("read outside");
        assert_eq!(kept, "UNTOUCHED");
    }
}
