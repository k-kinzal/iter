//! Recursive copy primitives used when a [`Mirror`](super::Mirror) is
//! materialised from a base directory.
//!
//! Traversal is iterative (explicit stack) so there is no risk of blowing
//! the async task stack on deeply nested trees. Asynchronous I/O uses
//! [`tokio::fs`] throughout.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::fs;

use super::filter::CloneFilter;
use super::mtime::{mtime, set_file_mtime};

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
) -> io::Result<()> {
    if !fs::try_exists(dst).await? {
        fs::create_dir_all(dst).await?;
    }
    let stamp_now = SystemTime::now();

    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((cur_src, cur_dst)) = stack.pop() {
        let mut entries = fs::read_dir(&cur_src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let rel = entry_path
                .strip_prefix(src)
                .expect("entries from a walk under `src` are always prefixed by `src`");
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

/// Copy a single file from `src` to `dst`, creating intermediate
/// directories as needed and correctly handling symlinks.
///
/// If `dst` already exists as a read-only regular file (e.g. a git
/// object or pack file, created with mode `0444`), `fs::copy` on Unix
/// fails with `EACCES` because it cannot open the destination for
/// writing. We transparently recover by unlinking the stale destination
/// and retrying the copy: the new file inherits `src`'s mode verbatim,
/// which is the behaviour rsync-style apply-back already promises.
pub(crate) async fn copy_file_preserving_parents(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).await?;
    }
    let meta = fs::symlink_metadata(src).await?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, dst).await?;
        return Ok(());
    }
    match fs::copy(src, dst).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            match fs::remove_file(dst).await {
                Ok(()) => {}
                Err(rm_err) if rm_err.kind() == io::ErrorKind::NotFound => return Err(e),
                Err(rm_err) => return Err(rm_err),
            }
            fs::copy(src, dst).await.map(|_| ())
        }
        Err(e) => Err(e),
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
