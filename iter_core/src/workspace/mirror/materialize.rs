//! Recursive copy primitives used when a [`Mirror`](super::Mirror) is
//! materialised from a base directory.
//!
//! Traversal is iterative (explicit stack) so there is no risk of blowing
//! the async task stack on deeply nested trees. Asynchronous I/O uses
//! [`tokio::fs`] throughout.

use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use super::filter::CloneFilter;
use super::mtime::{mtime, set_file_mtime};
use crate::time::Clock;

/// Recursively copy the contents of `src` into `dst`.
///
/// `dst` is created (including any missing parents) if it does not already
/// exist. Entries whose path *relative to `src`* is excluded by `filter` are
/// skipped — for directories this means the whole subtree is skipped (the
/// filter's auto-synthesised `<P>/**` ensures descendants are also matched).
///
/// `preserve_mtime` controls how the destination files' modification times
/// are set after copying:
///
/// - `true` — explicitly copy each source mtime onto the destination so the
///   result is platform-independent and stable across reads.
/// - `false` — set every destination mtime to "now" so the clone looks
///   freshly created. This is useful when the agent should not be able to
///   infer activity history from file timestamps.
///
/// Symlinks are copied by value: a symlink in `src` becomes a symlink in
/// `dst` pointing at the same target (see [`copy_symlink`]). File
/// permissions are preserved via [`fs::copy`]; symlinks' own mtimes are
/// left untouched in either mode (they reflect the link target on most
/// platforms).
pub(crate) async fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    filter: &CloneFilter,
    preserve_mtime: bool,
    clock: &dyn Clock,
) -> io::Result<()> {
    if !fs::try_exists(dst).await? {
        fs::create_dir_all(dst).await?;
    }
    let stamp_now = clock.system_time();

    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((cur_src, cur_dst)) = stack.pop() {
        let mut entries = fs::read_dir(&cur_src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            // Lexical `dir.join(name)` under `src`, so this strip never fails
            // in practice; return an I/O error rather than `expect`-panicking
            // (an unwind here would drop the live `TempDir` inline, running
            // `remove_dir_all` on the reactor thread — see `enumerate.rs`).
            let rel = entry_path.strip_prefix(src).map_err(|_| {
                io::Error::other(format!(
                    "mirror walk entry {} escaped source {}",
                    entry_path.display(),
                    src.display()
                ))
            })?;
            if filter.is_excluded(rel) {
                continue;
            }
            let file_type = entry.file_type().await?;
            let dst_entry = cur_dst.join(entry.file_name());
            if file_type.is_dir() {
                fs::create_dir_all(&dst_entry).await?;
                stack.push((entry_path, dst_entry));
            } else if file_type.is_symlink() {
                copy_symlink(&entry_path, &dst_entry).await?;
            } else if file_type.is_file() {
                if let Some(parent) = dst_entry.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::copy(&entry_path, &dst_entry).await?;
                let target = if preserve_mtime {
                    mtime(&entry_path).await?
                } else {
                    stamp_now
                };
                set_file_mtime(&dst_entry, target).await?;
            }
        }
    }
    Ok(())
}

/// Copy a single file into the base tree at `base.join(rel)`, creating any
/// missing parent directories and correctly handling symlinks.
///
/// This is the sole write primitive for apply-back, which writes
/// agent-influenced content back into the *persistent* base tree host-side,
/// after the sandbox is gone. The agent controls the temp tree's shape, and
/// a prior iteration (or a benign repo symlink) may have left a symlink in
/// the base at a path the temp tree now has as a regular file or a real
/// directory. A naive `fs::copy`/`create_dir_all` would *follow* such a link
/// and write through it — potentially to a target outside `base`. To keep
/// every write confined to `base`, this function never follows a symlink
/// when writing:
///
/// * Each parent directory of the target is materialised as a *real*
///   directory inside `base`; a symlink encountered along the relative chain
///   is unlinked and replaced with a real directory (see
///   [`ensure_base_dir_chain`]). This closes the intermediate-symlink escape
///   hatch that a benign-looking directory component can hide.
/// * If the final target `dst` is itself a stale symlink, it is unlinked
///   before the copy so [`fs::copy`] writes a fresh regular file in place
///   rather than following the link and truncating its target. This mirrors
///   the defensive unlink [`copy_symlink`] already performs for its branch.
///
/// If `dst` already exists as a read-only regular file (e.g. a git object or
/// pack file, created with mode `0444`), `fs::copy` on Unix fails with
/// `EACCES` because it cannot open the destination for writing. We
/// transparently recover by unlinking the stale destination and retrying the
/// copy: the new file inherits `src`'s mode verbatim, which is the behaviour
/// rsync-style apply-back already promises.
pub(crate) async fn copy_file_preserving_parents(
    base: &Path,
    rel: &Path,
    src: &Path,
) -> io::Result<()> {
    ensure_base_dir_chain(base, rel).await?;
    let dst = base.join(rel);

    let meta = fs::symlink_metadata(src).await?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, &dst).await?;
        return Ok(());
    }

    // `dst` may be a stale symlink the base retained from a prior iteration
    // (or a benign repo symlink) whose temp counterpart is now a regular
    // file. `fs::copy` would follow it and truncate the link's target — which
    // may be outside `base`. Unlink the link first so the copy writes a fresh
    // regular file in place, matching the temp tree.
    unlink_if_symlink(&dst).await?;

    match fs::copy(src, &dst).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            match fs::remove_file(&dst).await {
                Ok(()) => {}
                Err(rm_err) if rm_err.kind() == io::ErrorKind::NotFound => return Err(e),
                Err(rm_err) => return Err(rm_err),
            }
            fs::copy(src, &dst).await.map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// Materialise every parent directory of `base.join(rel)` as a *real*
/// directory inside `base`, never following a symlink out of `base`.
///
/// Walks the components of `rel` (all but the final file component) top-down
/// from `base`. A component that is already a real directory is left alone; a
/// component that is a symlink is unlinked (removing only the link, never its
/// target) and recreated as a real directory, so a stale or hostile
/// intermediate link cannot redirect a later write outside `base`. Missing
/// components are created. A pre-existing regular file where a directory must
/// go is left for `create_dir` to reject, preserving the prior
/// `create_dir_all` behaviour for that structural conflict.
async fn ensure_base_dir_chain(base: &Path, rel: &Path) -> io::Result<()> {
    let mut cur = base.to_path_buf();
    let mut comps = rel.components().peekable();
    while let Some(comp) = comps.next() {
        // Only the *parent* components need to be directories; stop before
        // the final (file) component, which the caller writes itself.
        if comps.peek().is_none() {
            break;
        }
        cur.push(comp);
        match fs::symlink_metadata(&cur).await {
            // `symlink_metadata` never follows, so each arm sees the link's
            // own type. For an lstat `FileType`, `is_symlink()` and `is_dir()`
            // are mutually exclusive on every platform (`is_dir()` is defined
            // to be false whenever `is_symlink()` is set), so the arm order is
            // not load-bearing — the symlink arm is written first only to keep
            // the non-following removal adjacent to its rationale. A symlinked
            // ancestor is unlinked (never its target — `remove_symlink` handles
            // Windows directory links correctly) and rebuilt as a real
            // directory, so it cannot redirect a later write out of `base`.
            Ok(meta) if meta.file_type().is_symlink() => {
                remove_symlink(&cur, meta.file_type()).await?;
                fs::create_dir(&cur).await?;
            }
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => {
                // A regular file where a directory must be: preserve prior
                // behaviour and let `create_dir` surface the conflict.
                fs::create_dir(&cur).await?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&cur).await?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// If `path` is itself a symbolic link, remove *only the link* (never its
/// target). A non-symlink or missing path is left untouched. The lstat never
/// dereferences the link.
async fn unlink_if_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => remove_symlink(path, meta.file_type()).await,
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove a symbolic link, dropping only the link itself and never its
/// target. `file_type` must come from a non-following lstat of the link.
///
/// On Unix `remove_file` unlinks any symlink regardless of the target kind, so
/// the link — never its target — is what disappears.
#[cfg(not(windows))]
pub(super) async fn remove_symlink(path: &Path, _file_type: std::fs::FileType) -> io::Result<()> {
    fs::remove_file(path).await
}

/// Windows variant: a *directory* symlink or junction must be removed with
/// `remove_dir` (`RemoveDirectoryW`), which drops the reparse point without
/// recursing into the target — `remove_file` (`DeleteFileW`) rejects anything
/// carrying the directory attribute. The directory case is detected with
/// [`std::os::windows::fs::FileTypeExt::is_symlink_dir`], not `is_dir()`: a
/// non-following lstat sets `is_symlink()`, and `is_dir()` is defined to be
/// `false` whenever `is_symlink()` is set, so `is_dir()` can never distinguish
/// a directory link here.
#[cfg(windows)]
pub(super) async fn remove_symlink(path: &Path, file_type: std::fs::FileType) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    if file_type.is_symlink_dir() {
        fs::remove_dir(path).await
    } else {
        fs::remove_file(path).await
    }
}

/// Copy a symlink from `src` to `dst`, preserving its target.
///
/// On Unix this uses [`tokio::fs::symlink`]. On Windows, the symlink is
/// recreated using [`std::os::windows::fs::symlink_file`] or `symlink_dir`
/// depending on the link target, falling back to a best-effort file copy if
/// neither is available.
#[cfg(unix)]
pub(crate) async fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = fs::read_link(src).await?;
    match fs::symlink_metadata(dst).await {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                fs::remove_dir_all(dst).await?;
            } else {
                fs::remove_file(dst).await?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::symlink(target, dst).await
}

/// Windows variant of [`copy_symlink`].
#[cfg(windows)]
pub(crate) async fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let target = fs::read_link(src).await?;
    match fs::symlink_metadata(dst).await {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_symlink_dir() {
                // A stale directory symlink/junction: drop only the reparse
                // point, never recurse into its (possibly out-of-base) target.
                // `is_dir()` is false for a link, so it cannot select this.
                fs::remove_dir(dst).await?;
            } else if ft.is_dir() {
                // A real directory: remove it and its contents.
                fs::remove_dir_all(dst).await?;
            } else {
                fs::remove_file(dst).await?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let resolved = if target.is_absolute() {
        target.clone()
    } else {
        src.parent()
            .map(|p| p.join(&target))
            .unwrap_or_else(|| target.clone())
    };
    let is_dir = fs::metadata(&resolved)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let target_clone = target.clone();
    let dst_owned = dst.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if is_dir {
            std::os::windows::fs::symlink_dir(&target_clone, &dst_owned)
        } else {
            std::os::windows::fs::symlink_file(&target_clone, &dst_owned)
        }
    })
    .await
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
}
