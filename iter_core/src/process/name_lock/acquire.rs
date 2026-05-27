//! `acquire` — the publish loop.
//!
//! ```text
//!   1. openat(.<name>.<hex>.tmp, O_CREAT|O_EXCL|O_RDWR|O_CLOEXEC|O_NOFOLLOW, 0600)
//!   2. write_all(body) + fsync(fd)
//!   3. flock(fd, LOCK_EX)            (uncontended on a fresh tmp)
//!   4. linkat(tmp → name, 0)         (create-fail-if-exists)
//!   5. on success: unlinkat(tmp) + fsync(dirfd) + return LockGuard
//!   6. on EEXIST: stale_check, may unlink-and-retry
//!   7. on ENOENT (janitor stole tmp): retry up to LINKAT_RETRY_MAX
//!   8. on EPERM/EOPNOTSUPP/EXDEV: UnsupportedFilesystem
//! ```

use std::io;
#[cfg(not(unix))]
use std::os::fd::BorrowedFd;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::Path;

use chrono::Utc;

use crate::process::error::RegistryError;
use crate::process::id::ProcessId;

use super::guard::LockGuard;
#[cfg(unix)]
use super::janitor::janitor_sweep;
#[cfg(unix)]
use super::name::tmp_name_format;
use super::name::validate_name;
#[cfg(unix)]
use super::stale::{StaleResolution, stale_check};
#[cfg(unix)]
use super::syscall::{
    best_effort_unlink, c_string, csprng_hex_16, dup_dirfd_cloexec, flock_exclusive, fstat_raw,
    fsync_dirfd, write_then_sync,
};

/// Maximum number of `linkat` retries when the tmp file vanishes between
/// `openat` and `linkat` (janitor race).
const LINKAT_RETRY_MAX: usize = 3;

/// Acquire `.locks/<name>` for `ulid`.
///
/// `locks_dirfd` is borrowed; the returned [`LockGuard`] dups it via
/// `F_DUPFD_CLOEXEC` so it can outlive the borrow.
///
/// `locks_dir_path` is used by the tmp-janitor sweep for `read_dir`; every
/// subsequent file mutation goes through `locks_dirfd` so the path is only
/// trusted for enumeration, not for modification.
///
/// `proc_root` is the parent of `<id>/status` files; `stale_check` reads
/// `<proc_root>/<ulid>/status` to decide whether a stale lock can be
/// recovered.
#[cfg(unix)]
pub(crate) fn acquire(
    locks_dirfd: BorrowedFd<'_>,
    locks_dir_path: &Path,
    proc_root: &Path,
    name: &str,
    ulid: ProcessId,
) -> Result<LockGuard, RegistryError> {
    validate_name(name)?;
    janitor_sweep(locks_dirfd, locks_dir_path);

    let body = format!("{}\n{}\n", ulid, Utc::now().to_rfc3339());
    let cname = c_string(name)?;

    for _retry in 0..LINKAT_RETRY_MAX {
        let suffix = csprng_hex_16();
        let tmp_name = tmp_name_format(name, &suffix);
        let ctmp = c_string(&tmp_name)?;

        // SAFETY: `locks_dirfd` is a valid kernel file descriptor for the
        // lifetime of the borrow; `ctmp.as_ptr()` is a valid NUL-terminated
        // C string. `openat` has no other preconditions.
        let raw_fd = unsafe {
            libc::openat(
                locks_dirfd.as_raw_fd(),
                ctmp.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if raw_fd < 0 {
            return Err(RegistryError::Io(io::Error::last_os_error()));
        }
        // SAFETY: `raw_fd` was just returned by a successful `openat` and is
        // not owned by any other handle — taking ownership here is sound.
        let owned: OwnedFd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let tmp_fd_raw = owned.as_raw_fd();

        if let Err(e) = write_then_sync(&owned, body.as_bytes()) {
            best_effort_unlink(locks_dirfd, &ctmp);
            return Err(RegistryError::Io(e));
        }
        if let Err(e) = flock_exclusive(tmp_fd_raw) {
            best_effort_unlink(locks_dirfd, &ctmp);
            return Err(RegistryError::Io(e));
        }

        // SAFETY: `locks_dirfd` is a valid kernel file descriptor for the
        // lifetime of the borrow; both `ctmp.as_ptr()` and `cname.as_ptr()`
        // are valid NUL-terminated C strings. `linkat` has no other
        // preconditions.
        let r = unsafe {
            libc::linkat(
                locks_dirfd.as_raw_fd(),
                ctmp.as_ptr(),
                locks_dirfd.as_raw_fd(),
                cname.as_ptr(),
                0,
            )
        };

        if r == 0 {
            best_effort_unlink(locks_dirfd, &ctmp);
            // The `linkat` made the lock entry visible. `fsync(dirfd)` is
            // what makes it durable across a kernel crash; if it fails we
            // surface the error rather than masking it, mirroring
            // `pid_file/publish.rs::PublishStep::FsyncDir`.
            fsync_dirfd(locks_dirfd).map_err(RegistryError::Io)?;
            let st = fstat_raw(tmp_fd_raw)?;
            let dirfd_dup = dup_dirfd_cloexec(locks_dirfd)?;
            return Ok(LockGuard {
                _fd: owned,
                locks_dirfd: dirfd_dup,
                name: name.to_owned(),
                identity: (st.st_dev, st.st_ino),
            });
        }

        let errno = io::Error::last_os_error();
        best_effort_unlink(locks_dirfd, &ctmp);
        drop(owned);

        match errno.raw_os_error() {
            Some(libc::EEXIST) => match stale_check(locks_dirfd, proc_root, &cname)? {
                StaleResolution::Recovered => {}
                StaleResolution::Live => return Err(RegistryError::AlreadyExists),
            },
            Some(libc::ENOENT) => {}
            Some(libc::EPERM | libc::EOPNOTSUPP | libc::EXDEV) => {
                return Err(RegistryError::UnsupportedFilesystem);
            }
            _ => return Err(RegistryError::Io(errno)),
        }
    }
    Err(RegistryError::TmpRetryExhausted)
}

#[cfg(not(unix))]
pub fn acquire(
    _locks_dirfd: BorrowedFd<'_>,
    _locks_dir_path: &Path,
    _proc_root: &Path,
    _name: &str,
    _ulid: ProcessId,
) -> Result<LockGuard, RegistryError> {
    Err(RegistryError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "name_lock::acquire is unix-only",
    )))
}
