//! `flock(2)` RAII guard plus the proc-directory liveness probe used by
//! [`super::ProcessStatusFile`] to bail out of a critical section when the
//! enclosing `<dir>` has been unlinked.
//!
//! Kept private to the `status_file` parent (`pub(super)`) because the
//! locking discipline is part of `ProcessStatusFile`'s contract and must
//! not leak into the wider `process::` surface.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

/// RAII wrapper around `flock(2)`. Holds a `RawFd` *copy* (not a borrow), so
/// callers can use the underlying `&mut File` freely inside the critical
/// section without borrow-checker collisions. The `Drop` impl runs
/// `flock(LOCK_UN)` if [`FlockGuard::release`] was not explicitly called
/// (panic path). The guard never calls `close(2)` — fd ownership stays with
/// the surrounding `File`.
pub(super) struct FlockGuard {
    fd: RawFd,
    released: bool,
}

#[cfg(unix)]
impl FlockGuard {
    /// Acquire `LOCK_EX` on the file's fd; retries on `EINTR`.
    pub(super) fn acquire_exclusive(file: &File) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        flock_exclusive(fd)?;
        Ok(Self {
            fd,
            released: false,
        })
    }

    /// Release the lock explicitly so the caller can observe the I/O result.
    /// On `Drop` the same syscall runs unobserved.
    pub(super) fn release(mut self) -> io::Result<()> {
        flock_unlock(self.fd)?;
        self.released = true;
        Ok(())
    }
}

#[cfg(not(unix))]
impl FlockGuard {
    pub(super) fn acquire_exclusive(_file: &File) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "flock is unix-only",
        ))
    }
    pub(super) fn release(self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        if !self.released {
            #[cfg(unix)]
            drop(flock_unlock(self.fd));
        }
    }
}

#[cfg(unix)]
fn flock_exclusive(fd: RawFd) -> io::Result<()> {
    loop {
        // SAFETY: `fd` was passed in by the caller and is the raw fd of a
        // live `File`; `libc::flock` has no other preconditions.
        let r = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if r == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

#[cfg(unix)]
fn flock_unlock(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is the raw fd of the live `File` whose flock we are
    // releasing; `libc::flock` has no other preconditions.
    let r = unsafe { libc::flock(fd, libc::LOCK_UN) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// True when the proc directory referenced by `dirfd` has been unlinked.
///
/// Detected via `fstat`: `nlink == 0` is the canonical signal. Errors that
/// imply the fd no longer points at a live inode (`EBADF`, `ENODEV`,
/// `ESTALE`) are conservatively treated as "vanished".
#[cfg(unix)]
pub(super) fn proc_dir_vanished(dirfd: BorrowedFd<'_>) -> bool {
    // SAFETY: `libc::stat` is a C POD; an all-zero bit pattern is a valid
    // initial state. `fstat` overwrites it on success.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dirfd.as_raw_fd()` is a valid kernel file descriptor for the
    // lifetime of the borrow; `&mut st` points to a properly aligned
    // `libc::stat`.
    let r = unsafe { libc::fstat(dirfd.as_raw_fd(), &raw mut st) };
    if r == 0 {
        return st.st_nlink == 0;
    }
    let err = io::Error::last_os_error();
    matches!(
        err.raw_os_error(),
        Some(libc::EBADF | libc::ENODEV | libc::ESTALE)
    )
}

#[cfg(not(unix))]
pub(super) fn proc_dir_vanished(_dirfd: BorrowedFd<'_>) -> bool {
    false
}
