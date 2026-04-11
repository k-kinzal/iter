//! Internal libc wrappers shared between `acquire`, `stale`, `janitor`, and
//! `guard`. All `pub(super)`; the wider crate only sees the typed API in
//! the parent module.

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::time::{Duration, SystemTime};

use crate::process::error::RegistryError;

/// Hard cap for a `.locks/<name>` file body. Real bodies are
/// `<ulid>\n<rfc3339>\n` ≈ 60 bytes; anything larger is rejected as
/// malformed before allocating, mirroring the `MAX_PID_FILE_BYTES`
/// defence-in-depth in `pid_file::read`.
#[cfg(unix)]
pub(super) const MAX_LOCK_BODY_BYTES: usize = 512;

/// Bounded read of a lock-file body. Returns `Err(io::ErrorKind::InvalidData)`
/// when the file exceeds [`MAX_LOCK_BODY_BYTES`] so callers can treat the
/// excess body as a parse failure rather than allocating arbitrarily.
///
/// Borrows the fd via `ManuallyDrop<File>` (mirroring [`write_then_sync`]) so
/// the underlying `OwnedFd` — and therefore the held `flock(LOCK_EX)` — stays
/// alive across the read. Taking ownership instead would close the fd and
/// release the flock when the local `File` dropped, opening a TOCTOU window
/// where a rival could re-acquire and re-publish the lock between the read
/// and any subsequent `unlinkat`.
#[cfg(unix)]
pub(super) fn read_lock_body(fd: &OwnedFd) -> io::Result<String> {
    use std::mem::ManuallyDrop;
    // SAFETY: `fd.as_raw_fd()` is a valid open kernel file descriptor for the
    // lifetime of `fd`. `ManuallyDrop` prevents the constructed `File` from
    // closing the fd at scope end so `OwnedFd` retains sole ownership and
    // the held flock is preserved. Aliasing: the `OwnedFd` and the
    // `ManuallyDrop<File>` refer to the same kernel object, but `&OwnedFd`
    // does not expose `&File`; we take only one shared `&File` borrow
    // (`file_ref` below) to drive `Read::take` — no concurrent I/O paths
    // exist.
    let file = ManuallyDrop::new(unsafe { File::from_raw_fd(fd.as_raw_fd()) });
    let file_ref: &File = &file;
    let mut buf = Vec::with_capacity(64);
    file_ref
        .take(MAX_LOCK_BODY_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    if buf.len() > MAX_LOCK_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "lock body exceeds MAX_LOCK_BODY_BYTES",
        ));
    }
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Build a `CString` for the lock-file basename, mapping interior NULs to
/// [`RegistryError::InvalidName`] rather than letting libc reject the call.
pub(super) fn c_string(s: &str) -> Result<CString, RegistryError> {
    CString::new(s.as_bytes()).map_err(|_| RegistryError::InvalidName {
        reason: "embedded NUL byte".into(),
    })
}

/// Write `body` to `fd` and `fsync(fd)`. The fd is borrowed via
/// `ManuallyDrop<File>` so the underlying `OwnedFd` is not consumed and
/// CLOEXEC is preserved (a `libc::dup` clone would silently lose it).
#[cfg(unix)]
pub(super) fn write_then_sync(fd: &OwnedFd, body: &[u8]) -> io::Result<()> {
    use std::mem::ManuallyDrop;
    // SAFETY: `fd.as_raw_fd()` is a valid open kernel file descriptor for the
    // lifetime of `fd`. `ManuallyDrop` prevents the constructed `File` from
    // closing the fd at scope end so `OwnedFd` retains sole ownership.
    let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(fd.as_raw_fd()) });
    file.write_all(body)?;
    file.sync_all()?;
    Ok(())
}

/// 32-character lower-hex CSPRNG token used as the `.<name>.<32hex>.tmp`
/// suffix.
pub(super) fn csprng_hex_16() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(unix)]
pub(super) fn fstat_raw(fd: RawFd) -> Result<libc::stat, RegistryError> {
    // SAFETY: `libc::stat` is a C POD; an all-zero bit pattern is a valid
    // initial state. `fstat` overwrites it on success.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is the caller's responsibility to provide as a valid
    // descriptor; `&mut st` points to a properly aligned `libc::stat`.
    let r = unsafe { libc::fstat(fd, &raw mut st) };
    if r != 0 {
        return Err(RegistryError::Io(io::Error::last_os_error()));
    }
    Ok(st)
}

#[cfg(unix)]
pub(super) fn fstatat_nofollow(dirfd: RawFd, name: &CString) -> io::Result<libc::stat> {
    // SAFETY: see `fstat_raw` — zero is a valid `libc::stat` bit pattern.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dirfd` is the caller's responsibility; `name.as_ptr()` is a
    // valid NUL-terminated C string and `&mut st` points to a properly
    // aligned `libc::stat`.
    let r = unsafe { libc::fstatat(dirfd, name.as_ptr(), &raw mut st, libc::AT_SYMLINK_NOFOLLOW) };
    if r != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(st)
    }
}

/// `fsync(dirfd)` to make a directory entry change durable. Mirrors
/// `pid_file::syscall::fsync_dirfd` — the result MUST be propagated, not
/// dropped, so a crash between `linkat` publish and durability does not
/// silently lose the lock entry.
#[cfg(unix)]
pub(super) fn fsync_dirfd(dirfd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: `dirfd.as_raw_fd()` is a valid kernel file descriptor for the
    // lifetime of the borrow.
    let r = unsafe { libc::fsync(dirfd.as_raw_fd()) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// EINTR-resilient `flock(LOCK_EX)`.
#[cfg(unix)]
pub(super) fn flock_exclusive(fd: RawFd) -> io::Result<()> {
    loop {
        // SAFETY: `fd` is the caller's responsibility to provide as a valid
        // descriptor; `flock` has no other preconditions.
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

/// `fcntl(F_DUPFD_CLOEXEC, 0)` of the locks `dirfd` so the resulting
/// [`LockGuard`](super::guard::LockGuard) can outlive the borrow without
/// dropping CLOEXEC.
#[cfg(unix)]
pub(super) fn dup_dirfd_cloexec(borrowed: BorrowedFd<'_>) -> Result<OwnedFd, RegistryError> {
    // SAFETY: `borrowed.as_raw_fd()` is a valid kernel file descriptor for the
    // lifetime of the borrow.
    let r = unsafe { libc::fcntl(borrowed.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if r < 0 {
        return Err(RegistryError::Io(io::Error::last_os_error()));
    }
    // SAFETY: `r` was just returned by a successful `fcntl(F_DUPFD_CLOEXEC)`
    // and is not owned by any other handle.
    Ok(unsafe { OwnedFd::from_raw_fd(r) })
}

/// Discard the result of `unlinkat(name, 0)` — used in error-recovery paths
/// where the primary error must take precedence.
#[cfg(unix)]
pub(super) fn best_effort_unlink(dirfd: BorrowedFd<'_>, name: &CString) {
    // SAFETY: `dirfd` is a valid kernel file descriptor for the lifetime of
    // the borrow; `name.as_ptr()` is a valid NUL-terminated C string. The
    // syscall has no preconditions beyond the fd/path being valid, and we
    // intentionally discard its result.
    unsafe {
        libc::unlinkat(dirfd.as_raw_fd(), name.as_ptr(), 0);
    }
}

/// `unlinkat(dirfd, name, 0)` that treats `ENOENT` as success but propagates
/// every other errno. Used by recovery paths (`stale_check`, `release_by_id`)
/// where a persistent failure (e.g. `EPERM` on a read-only mount) must
/// surface rather than be silently swallowed and misreported downstream as
/// `TmpRetryExhausted`.
#[cfg(unix)]
pub(super) fn unlinkat_allow_enoent(dirfd: BorrowedFd<'_>, name: &CString) -> io::Result<()> {
    // SAFETY: `dirfd` is a valid kernel file descriptor for the lifetime of
    // the borrow; `name.as_ptr()` is a valid NUL-terminated C string.
    let r = unsafe { libc::unlinkat(dirfd.as_raw_fd(), name.as_ptr(), 0) };
    if r != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOENT) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

/// Wall-clock age of an inode's `mtime`, clamping pre-epoch / future-jumped
/// values to zero. Used by both the janitor and the corrupt-lock grace check.
#[cfg(unix)]
pub(super) fn lock_age(st: &libc::stat) -> Duration {
    let now = SystemTime::now();
    let mtime_secs = u64::try_from(st.st_mtime).unwrap_or(0);
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs);
    now.duration_since(then).unwrap_or_default()
}
