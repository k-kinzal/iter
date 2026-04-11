//! Modification-time helpers used by [`Mirror`](super::Mirror).
//!
//! Kept separate so the copy path (`materialize`) and the reconcile path
//! (`reconcile`) share the same async wrappers without growing each
//! module's imports.

use std::fs::FileTimes;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use tokio::fs;

/// Read the modified-time of a filesystem entry, returning
/// [`SystemTime::UNIX_EPOCH`] if the platform cannot report one.
pub(crate) async fn mtime(path: &Path) -> io::Result<SystemTime> {
    let meta = fs::symlink_metadata(path).await?;
    meta.modified().or(Ok(SystemTime::UNIX_EPOCH))
}

/// Set the modification time of a regular file.
///
/// Symlinks are followed: callers should only invoke this on regular files
/// (the copy traversal does so explicitly). Errors from the inner blocking
/// call are flattened back into the async context.
pub(crate) async fn set_file_mtime(path: &Path, time: SystemTime) -> io::Result<()> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&owned)?;
        let times = FileTimes::new().set_modified(time);
        file.set_times(times)
    })
    .await
    .map_err(io::Error::other)?
}
