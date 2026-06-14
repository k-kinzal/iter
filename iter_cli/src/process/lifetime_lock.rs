//! Per-record lifetime lock.
//!
//! The owner process holds an exclusive `flock` on `<proc>/<id>/lifetime.lock`
//! for as long as the session is alive. A separate CLI can then probe
//! liveness by attempting a non-blocking exclusive lock: `WouldBlock` means
//! the record is currently owned by a live process, while success means no
//! process holds the lifetime lock.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::process::error::ProcessError;
use crate::process::paths::{FILE_MODE, ProcPaths, names};

#[derive(Debug)]
pub(crate) struct LifetimeLock {
    file: File,
}

impl LifetimeLock {
    pub(crate) fn acquire(paths: &ProcPaths) -> Result<Self, ProcessError> {
        let path = paths.join(names::LIFETIME_LOCK);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .mode(FILE_MODE)
            .open(&path)
            .map_err(ProcessError::Io)?;
        flock_exclusive(file.as_raw_fd()).map_err(ProcessError::FlockAcquire)?;
        Ok(Self { file })
    }

    #[cfg(test)]
    pub(crate) fn raw_fd_for_test(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

impl Drop for LifetimeLock {
    fn drop(&mut self) {
        drop(flock_unlock(self.file.as_raw_fd()));
    }
}

pub(crate) fn is_contended(dir: &Path) -> Result<bool, ProcessError> {
    let path = dir.join(names::LIFETIME_LOCK);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(ProcessError::Io(e)),
    };
    match flock_try_exclusive(file.as_raw_fd()) {
        Ok(()) => {
            flock_unlock(file.as_raw_fd()).map_err(ProcessError::FlockRelease)?;
            Ok(false)
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(ProcessError::FlockAcquire(e)),
    }
}

#[cfg(unix)]
fn flock_exclusive(fd: std::os::fd::RawFd) -> io::Result<()> {
    loop {
        // SAFETY: `fd` is the raw fd of a live `File`.
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

#[cfg(not(unix))]
fn flock_exclusive(_fd: std::os::fd::RawFd) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock is unix-only",
    ))
}

#[cfg(unix)]
fn flock_try_exclusive(fd: std::os::fd::RawFd) -> io::Result<()> {
    loop {
        // SAFETY: `fd` is the raw fd of a live `File`.
        let r = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
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

#[cfg(not(unix))]
fn flock_try_exclusive(_fd: std::os::fd::RawFd) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock is unix-only",
    ))
}

#[cfg(unix)]
fn flock_unlock(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: `fd` is the raw fd of a live `File`.
    let r = unsafe { libc::flock(fd, libc::LOCK_UN) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn flock_unlock(_fd: std::os::fd::RawFd) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessId;
    use crate::process::paths::ProcPaths;
    use tempfile::TempDir;

    #[test]
    fn contended_while_lifetime_lock_is_held() {
        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        let lock = LifetimeLock::acquire(&paths).expect("acquire");
        assert!(lock.raw_fd_for_test() >= 0);

        assert!(is_contended(paths.dir()).expect("probe"));

        drop(lock);
        assert!(!is_contended(paths.dir()).expect("probe after drop"));
    }

    #[test]
    fn missing_lifetime_lock_is_not_contended() {
        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        assert!(!is_contended(paths.dir()).expect("probe"));
    }
}
