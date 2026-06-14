//! `release_by_id` — unlink `.locks/<name>` when its body matches a known
//! [`ProcessId`].
//!
//! Used by [`crate::process::handle::ProcessHandle::remove`] after the
//! original `LockGuard` has been dropped (and therefore the in-memory flock
//! is gone). Mirrors the post-flock `(st_dev, st_ino)` re-validation used by
//! [`super::stale::stale_check`] so a parallel acquirer that already
//! reclaimed the slot is never disturbed.

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use crate::process::error::RegistryError;
use crate::process::id::ProcessId;

#[cfg(unix)]
use super::name::validate_name;
#[cfg(unix)]
use super::syscall::{
    c_string, flock_exclusive, fstat_raw, fstatat_nofollow, read_lock_body, unlinkat_allow_enoent,
};

/// Best-effort release of `.locks/<name>` when its body's first line decodes
/// to `expected_id`.
///
/// `Ok(())` is returned in every "lock is no longer ours" shape:
/// - the lock file does not exist,
/// - it was unlinked + recreated under a fresh inode after our open,
/// - the body's ULID does not match `expected_id`.
///
/// I/O errors propagate as [`RegistryError::Io`]. A body that is malformed
/// or oversized parses to a non-matching ULID and is therefore left alone —
/// the corrupt-body grace path in `acquire`/`stale_check` will handle it.
#[cfg(unix)]
pub(crate) fn release_by_id(
    locks_dirfd: BorrowedFd<'_>,
    name: &str,
    expected_id: ProcessId,
) -> Result<(), RegistryError> {
    validate_name(name)?;
    let cname = c_string(name)?;

    // SAFETY: `locks_dirfd` is a valid kernel file descriptor for the lifetime
    // of the borrow; `cname.as_ptr()` is a valid NUL-terminated C string.
    let raw_fd = unsafe {
        libc::openat(
            locks_dirfd.as_raw_fd(),
            cname.as_ptr(),
            libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw_fd < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOENT) {
            return Ok(());
        }
        return Err(RegistryError::Io(err));
    }
    // SAFETY: `raw_fd` was returned by a successful `openat` above and is not
    // owned by any other handle — taking ownership here is sound.
    let fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let st_a = fstat_raw(fd.as_raw_fd())?;

    flock_exclusive(fd.as_raw_fd()).map_err(RegistryError::Io)?;

    // Re-stat through the dirfd to detect "unlink + recreate" between open
    // and flock — same pattern as `stale_check`.
    let st_b = match fstatat_nofollow(locks_dirfd.as_raw_fd(), &cname) {
        Ok(s) => s,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
        Err(e) => return Err(RegistryError::Io(e)),
    };
    if (st_a.st_dev, st_a.st_ino) != (st_b.st_dev, st_b.st_ino) {
        return Ok(());
    }

    // Bounded read so a malicious oversized body cannot drive an unbounded
    // allocation. `InvalidData` (oversize / non-utf8) is treated the same
    // as a non-matching body: we leave the lock alone. We borrow the fd —
    // the `OwnedFd` (and therefore the held flock) must stay alive until
    // after `unlinkat_allow_enoent` runs to close the TOCTOU window.
    let body = match read_lock_body(&fd) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => return Ok(()),
        Err(e) => return Err(RegistryError::Io(e)),
    };
    if !body_matches(&body, expected_id) {
        return Ok(());
    }

    unlinkat_allow_enoent(locks_dirfd, &cname).map_err(RegistryError::Io)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn release_by_id(
    _locks_dirfd: std::os::fd::BorrowedFd<'_>,
    _name: &str,
    _expected_id: ProcessId,
) -> Result<(), RegistryError> {
    Ok(())
}

fn body_matches(body: &str, expected_id: ProcessId) -> bool {
    body.lines()
        .next()
        .and_then(|line| line.parse::<ProcessId>().ok())
        .is_some_and(|id| id == expected_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_matches_first_line_only() {
        let id = ProcessId::generate();
        let body = format!("{id}\n2026-04-26T00:00:00Z\n");
        assert!(body_matches(&body, id));
    }

    #[test]
    fn body_matches_rejects_other_id() {
        let a = ProcessId::generate();
        let b = ProcessId::generate();
        let body = format!("{a}\n2026-04-26T00:00:00Z\n");
        assert!(!body_matches(&body, b));
    }

    #[test]
    fn body_matches_rejects_empty() {
        assert!(!body_matches("", ProcessId::generate()));
    }
}
