//! Empty-directory cleanup used by the
//! [`Sync`](crate::workspace::ApplyBackMode::Sync) apply-back mode.
//!
//! After removing files that no longer exist in the temp workspace,
//! `Sync` may leave behind empty directories in the base tree. This
//! module strips them without touching non-empty dirs, excluded
//! subtrees, or the root itself.

use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::filter::ApplyBackFilter;

/// Recursively remove any *empty* directories inside `root`, leaving `root`
/// itself in place.
///
/// This is a best-effort cleanup used by the [`Sync`](crate::workspace::ApplyBackMode::Sync)
/// apply-back mode after removing files that no longer exist in the temp
/// workspace. Directories matched by `filter` are skipped entirely so the
/// walk never enters subtrees the filter wants left alone.
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
            let rel = p
                .strip_prefix(root)
                .expect("entries from a walk under `root` are always prefixed by `root`");
            if filter.is_excluded(rel) {
                continue;
            }
            if entry.file_type().await?.is_dir() {
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
