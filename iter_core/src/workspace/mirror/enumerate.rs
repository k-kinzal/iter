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
            // `entry.path()` is a lexical `dir.join(name)` under `root`, so
            // this strip never fails in practice. Surface a stray invariant
            // break as an I/O error rather than `expect`-panicking: a panic
            // here would unwind and drop the live `TempDir` inline, running
            // `remove_dir_all` on the reactor thread (the very thing
            // `Mirror::close_best_effort` routes through `spawn_blocking`).
            let rel = entry_path.strip_prefix(root).map_err(|_| {
                io::Error::other(format!(
                    "mirror walk entry {} escaped root {}",
                    entry_path.display(),
                    root.display()
                ))
            })?;
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                // Descent and leaf-masking are distinct questions: a directory
                // the leaf filter excludes may still hold a negation/whitelist
                // re-include (e.g. `["vendor", "!vendor/keep"]`), so gate
                // descent on `should_descend`, not `is_excluded`. Pruning here
                // on `is_excluded` would leave the exception unreachable.
                if filter.should_descend(rel) {
                    stack.push(entry_path);
                }
            } else if !filter.is_excluded(rel) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}
