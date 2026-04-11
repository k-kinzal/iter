//! Internal syscall wrappers shared by `publish`, `cleanup`, and `read`.
//!
//! These are intentionally `pub(super)` only — no `pub(crate)` re-export.
//! The `pid_file` module owns the on-disk representation; everything outside
//! it talks through the typed API in [`super::publish`], [`super::read`],
//! and [`super::cleanup`].

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};

/// EINTR-resilient `write_all` against a raw fd.
#[cfg(unix)]
pub(super) fn write_all(fd: &OwnedFd, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        let n = usize::try_from(n).expect("write returned non-negative ssize_t");
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
        }
        buf = &buf[n..];
    }
    Ok(())
}

/// `fsync(fd)` on a regular file fd.
#[cfg(unix)]
pub(super) fn fsync_fd(fd: &OwnedFd) -> io::Result<()> {
    let r = unsafe { libc::fsync(fd.as_raw_fd()) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `fsync(dirfd)` to make a directory entry change durable.
#[cfg(unix)]
pub(super) fn fsync_dirfd(dirfd: BorrowedFd<'_>) -> io::Result<()> {
    let r = unsafe { libc::fsync(dirfd.as_raw_fd()) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `unlinkat(dirfd, name, 0)`. Caller decides how to treat `ENOENT`.
#[cfg(unix)]
pub(super) fn unlinkat_name(dirfd: BorrowedFd<'_>, name: &str) -> io::Result<()> {
    let cstr =
        std::ffi::CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let r = unsafe { libc::unlinkat(dirfd.as_raw_fd(), cstr.as_ptr(), 0) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
