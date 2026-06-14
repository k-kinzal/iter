//! Residue cleanup helpers for the pid-file directory.
//!
//! These run **under the `<dir>/status` flock** held by
//! `reconcile_under_lock`, so concurrent publishers cannot race the
//! unconditional delete of `.pid.tmp`. The shared
//! [`pid_residue_predicate`] is also used by `read()` to recognise
//! `linkat` partial-adoption residue (`nlink == 2` + `.pid.tmp` matching by
//! `(st_dev, st_ino)`).

use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;

#[cfg(unix)]
use crate::process::paths::names;

#[cfg(unix)]
use super::syscall::unlinkat_name;

/// Predicate shared between `read()` and the cleanup helpers in
/// `reconcile_under_lock` so the "is this a `linkat` residue?" decision uses
/// exactly one definition.
///
/// Returns `true` only when `pid` has `nlink == 2` *and* `.pid.tmp` exists
/// *and* `(st_dev, st_ino)` of `.pid.tmp` matches `pid`'s. `nlink ≥ 3` is
/// always `false`: `linkat`-only publication can produce at most two links.
#[cfg(unix)]
pub(crate) fn pid_residue_predicate(dirfd: BorrowedFd<'_>) -> bool {
    let dirfd_raw = dirfd.as_raw_fd();

    // SAFETY: `libc::stat` is a C POD and all-zero bytes are a valid initial
    // value before `fstatat` fills the structure.
    let mut pid_st: libc::stat = unsafe { std::mem::zeroed() };
    let Ok(pid_name) = std::ffi::CString::new(names::PID) else {
        return false;
    };
    // SAFETY: `dirfd_raw` is copied from a live borrowed directory fd,
    // `pid_name` is NUL-terminated, and `pid_st` is writable for the call.
    let r = unsafe {
        libc::fstatat(
            dirfd_raw,
            pid_name.as_ptr(),
            &raw mut pid_st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if r != 0 || pid_st.st_nlink != 2 {
        return false;
    }

    // SAFETY: `libc::stat` is a C POD and all-zero bytes are a valid initial
    // value before `fstatat` fills the structure.
    let mut tmp_st: libc::stat = unsafe { std::mem::zeroed() };
    let Ok(tmp_name) = std::ffi::CString::new(names::PID_TMP) else {
        return false;
    };
    // SAFETY: `dirfd_raw` is copied from a live borrowed directory fd,
    // `tmp_name` is NUL-terminated, and `tmp_st` is writable for the call.
    let r = unsafe {
        libc::fstatat(
            dirfd_raw,
            tmp_name.as_ptr(),
            &raw mut tmp_st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if r != 0 {
        return false;
    }
    tmp_st.st_dev == pid_st.st_dev && tmp_st.st_ino == pid_st.st_ino
}

#[cfg(not(unix))]
pub(crate) fn pid_residue_predicate(_dirfd: BorrowedFd<'_>) -> bool {
    false
}

/// Best-effort `unlinkat(.pid.tmp)`. Used by `reconcile_under_lock` to clean
/// the residue left by a `linkat` success that was followed by a crash before
/// `unlinkat` could run. `ENOENT` is reported as `Ok(())` since absence is the
/// desired post-condition.
#[cfg(unix)]
pub(crate) fn delete_pid_tmp(dirfd: BorrowedFd<'_>) -> io::Result<()> {
    match unlinkat_name(dirfd, names::PID_TMP) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
pub(crate) fn delete_pid_tmp(_dirfd: BorrowedFd<'_>) -> io::Result<()> {
    Ok(())
}
