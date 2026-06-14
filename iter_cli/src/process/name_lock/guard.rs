//! `LockGuard` — owned handle for an active name registration.
//!
//! Holds the locked file descriptor; dropping it auto-releases the kernel
//! `flock`. To also unlink the on-disk entry, call [`LockGuard::release`]
//! (typically from `Handle::remove()`).

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};

#[cfg(unix)]
use super::syscall::{fstatat_nofollow, unlinkat_allow_enoent};

/// Owned handle for an active name registration.
#[derive(Debug)]
pub(crate) struct LockGuard {
    /// Held for its `Drop` side-effect: closing the fd releases the kernel
    /// `flock(LOCK_EX)` acquired at registration time. The leading
    /// underscore opts out of the dead-code lint without a suppression
    /// attribute, signalling at the field name that the value is never read.
    #[cfg(unix)]
    pub(super) _fd: OwnedFd,
    #[cfg(unix)]
    pub(super) locks_dirfd: OwnedFd,
    pub(super) name: String,
    /// `(st_dev, st_ino)` snapshot at acquisition. `release` verifies the
    /// on-disk entry still matches before unlinking, in case a parallel
    /// acquirer recovered the slot under us.
    #[cfg(unix)]
    pub(super) identity: (libc::dev_t, libc::ino_t),
}

impl LockGuard {
    /// Borrow of the held name (the on-disk file's basename under `.locks/`).
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Verify the on-disk inode still matches what we created, then
    /// `unlinkat`. ENOENT is treated as success (parallel acquirer already
    /// ran a stale recovery on us).
    #[cfg(unix)]
    pub(crate) fn release(self) -> io::Result<()> {
        let Ok(cname) = CString::new(self.name.as_bytes()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "name contains a NUL byte",
            ));
        };
        // SAFETY: `self.locks_dirfd: OwnedFd` is moved into this function by
        // `self`, is not dropped or moved before the function returns, and
        // therefore outlives the `BorrowedFd<'_>` returned here as well as
        // every borrow it feeds (the `fstatat_nofollow` and
        // `unlinkat_allow_enoent` calls below). The `borrow_raw` precondition
        // — that the raw fd remains valid for the borrow's lifetime — is
        // satisfied by that ownership window.
        let dirfd = unsafe { BorrowedFd::borrow_raw(self.locks_dirfd.as_raw_fd()) };
        match fstatat_nofollow(dirfd.as_raw_fd(), &cname) {
            Ok(st) => {
                if (st.st_dev, st.st_ino) != self.identity {
                    return Ok(());
                }
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
            Err(e) => return Err(e),
        }
        unlinkat_allow_enoent(dirfd, &cname)
    }

    #[cfg(not(unix))]
    pub(crate) fn release(self) -> io::Result<()> {
        Ok(())
    }
}
