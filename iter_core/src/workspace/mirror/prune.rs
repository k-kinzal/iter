//! Empty-directory cleanup used by the
//! [`Sync`](crate::workspace::ApplyBackMode::Sync) apply-back mode.
//!
//! After removing files that no longer exist in the temp workspace,
//! `Sync` may leave behind empty directories in the base tree. This
//! module strips them without touching non-empty dirs or the root
//! itself. It walks the same subtrees the reconcile enumerate/delete
//! phase did (via [`ApplyBackFilter::should_descend`]), so a directory
//! the delete phase just emptied is reclaimed even when it sits inside
//! an otherwise-excluded subtree a negation or whitelist reached into;
//! a plain excluded subtree (no negation, no whitelist) is still pruned
//! wholesale and left alone.

use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::filter::ApplyBackFilter;

/// Recursively remove any *empty* directories inside `root`, leaving `root`
/// itself in place.
///
/// This is a best-effort cleanup used by the [`Sync`](crate::workspace::ApplyBackMode::Sync)
/// apply-back mode after removing files that no longer exist in the temp
/// workspace. The walk descends exactly where
/// [`ApplyBackFilter::should_descend`] allows — mirroring the enumerate and
/// delete phases — so it reclaims a directory the delete phase just emptied
/// even inside an excluded subtree a negation or whitelist reached into,
/// while a plain excluded subtree is pruned wholesale and left untouched.
/// [`fs::remove_dir`] no-ops on any directory that still holds content, so
/// descending never removes a directory the operator's data lives in.
pub(crate) async fn prune_empty_dirs(root: &Path, filter: &ApplyBackFilter) -> io::Result<()> {
    if !fs::try_exists(root).await? {
        return Ok(());
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let p = entry.path();
            // Lexical `dir.join(name)` under `root`, so this strip never fails
            // in practice; return an I/O error rather than `expect`-panicking
            // (an unwind here would drop the live `TempDir` inline, running
            // `remove_dir_all` on the reactor thread — see `enumerate.rs`).
            let rel = p.strip_prefix(root).map_err(|_| {
                io::Error::other(format!(
                    "mirror walk entry {} escaped root {}",
                    p.display(),
                    root.display()
                ))
            })?;
            // Mirror the enumerate/delete walk in
            // `enumerate::list_files_relative`: descend wherever the reconcile
            // phase could have added or removed a file. The delete phase can
            // empty a negation-rescued file's parent *inside* an excluded
            // subtree (e.g. `["vendor", "!vendor/keep/sub.txt"]` once the temp
            // tree drops `sub.txt`); that now-empty dir must be reachable so it
            // can be reclaimed. `should_descend` still prunes a plain excluded
            // subtree wholesale, and `remove_dir` below no-ops on any directory
            // that still holds content, so the wider walk never removes a
            // directory the operator's data lives in.
            if entry.file_type().await?.is_dir() && filter.should_descend(rel) {
                stack.push(p.clone());
                dirs.push(p);
            }
        }
    }
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in dirs {
        match fs::remove_dir(&dir).await {
            Ok(()) => {}
            Err(e)
                if e.kind() == io::ErrorKind::DirectoryNotEmpty
                    || e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
