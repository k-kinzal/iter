//! Flat-list enumeration of the files inside a [`Mirror`](super::Mirror).
//!
//! Used during reconcile (apply-back) to diff the temp tree against the
//! base tree without having to walk both in lockstep.

use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::filter::ApplyBackFilter;

/// Flatten a directory tree into a sorted list of file paths *relative to*
/// `root`, honouring `filter`.
///
/// Directories themselves are not emitted — only files (regular files and
/// symlinks). The returned paths are suitable for direct joining onto either
/// the source or destination root.
pub(crate) async fn list_files_relative(
    root: &Path,
    filter: &ApplyBackFilter,
) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !fs::try_exists(root).await? {
        return Ok(out);
    }
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let rel = entry_path
                .strip_prefix(root)
                .expect("entries from a walk under `root` are always prefixed by `root`");
            if filter.is_excluded(rel) {
                continue;
            }
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(entry_path);
            } else {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}
