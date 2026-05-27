//! Stale-lock recovery: deciding whether an `EEXIST` collision can be
//! reclaimed for a fresh acquirer.
//!
//! `acquire` calls [`stale_check`] when `linkat(.tmp → name)` returns
//! `EEXIST`. We open + flock the existing entry, parse its body, and
//! consult the referenced `<id>/status` to decide:
//!
//! - body refers to a record whose status is `Initializing` or `Running`
//!   ⇒ `Live` (caller surfaces `AlreadyExists`).
//! - body parses but the record is terminal / missing ⇒ unlink and tell
//!   the caller to retry the publish loop.
//! - body fails to parse and the file is younger than the corrupt-grace
//!   ⇒ caller surfaces `CorruptLock` (gives a slow publisher a chance).
//! - body fails to parse and is older than the corrupt-grace ⇒ unlink
//!   and retry.

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::Path;

use chrono::Utc;

use crate::process::error::RegistryError;
use crate::process::id::ProcessId;
use crate::process::status::ProcessStatus;

#[cfg(unix)]
use super::syscall::{
    flock_exclusive, fstat_raw, fstatat_nofollow, lock_age, read_lock_body, unlinkat_allow_enoent,
};

/// Stale-lock corrupt-body grace window. A lock body that fails to parse is
/// not auto-recovered until its `mtime` is older than this — gives a slow
/// publish-after-write a chance to finish.
const CORRUPT_LOCK_GRACE_SECS: u64 = 60;

pub(super) enum StaleResolution {
    /// Lock was successfully unlinked; caller must restart the publish loop.
    Recovered,
    /// Lock body refers to a still-live `proc/<ulid>` record.
    Live,
}

#[cfg(unix)]
pub(super) fn stale_check(
    locks_dirfd: BorrowedFd<'_>,
    proc_root: &Path,
    cname: &CString,
) -> Result<StaleResolution, RegistryError> {
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
            return Ok(StaleResolution::Recovered);
        }
        return Err(RegistryError::Io(err));
    }
    // SAFETY: `raw_fd` was just returned by a successful `openat` and is not
    // owned by any other handle — taking ownership here is sound.
    let owned: OwnedFd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let st_a = fstat_raw(owned.as_raw_fd())?;

    flock_exclusive(owned.as_raw_fd()).map_err(RegistryError::Io)?;

    // After flock, re-stat via dirfd to detect "someone unlinked + recreated"
    // between our open and our flock acquisition.
    let st_b = match fstatat_nofollow(locks_dirfd.as_raw_fd(), cname) {
        Ok(s) => s,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(StaleResolution::Recovered);
        }
        Err(e) => return Err(RegistryError::Io(e)),
    };
    if (st_a.st_dev, st_a.st_ino) != (st_b.st_dev, st_b.st_ino) {
        return Ok(StaleResolution::Recovered);
    }

    // Bounded read: an oversized body is treated as a parse failure so
    // the corrupt-grace branch reclaims the slot without unbounded heap
    // allocation. Mirrors `pid_file::read::MAX_PID_FILE_BYTES`.
    // Borrow the fd; the `OwnedFd` (and therefore the held flock) must stay
    // alive until after the unlink branches below run.
    let parsed = match read_lock_body(&owned) {
        Ok(body) => parse_lock_body(&body).map(|(ulid, _)| ulid),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Err("body too large or non-utf8"),
        Err(e) => return Err(RegistryError::Io(e)),
    };

    match parsed {
        Err(_) => {
            let age = lock_age(&st_b);
            if age.as_secs() >= CORRUPT_LOCK_GRACE_SECS {
                unlinkat_allow_enoent(locks_dirfd, cname).map_err(RegistryError::Io)?;
                return Ok(StaleResolution::Recovered);
            }
            Err(RegistryError::CorruptLock)
        }
        Ok(ulid) => {
            if is_record_live(proc_root, ulid)? {
                Ok(StaleResolution::Live)
            } else {
                unlinkat_allow_enoent(locks_dirfd, cname).map_err(RegistryError::Io)?;
                Ok(StaleResolution::Recovered)
            }
        }
    }
}

fn parse_lock_body(body: &str) -> Result<(ProcessId, chrono::DateTime<Utc>), &'static str> {
    let mut lines = body.lines();
    let ulid_line = lines.next().ok_or("empty")?;
    let ts_line = lines.next().ok_or("missing timestamp")?;
    let ulid: ProcessId = ulid_line.parse().map_err(|_| "bad ulid")?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_line)
        .map_err(|_| "bad timestamp")?
        .with_timezone(&Utc);
    Ok((ulid, ts))
}

fn is_record_live(proc_root: &Path, ulid: ProcessId) -> Result<bool, RegistryError> {
    let path = proc_root.join(ulid.to_string()).join("status");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(RegistryError::Io(e)),
    };
    let token = raw.trim_end_matches(['\n', '\r']);
    let token_only = token.split_whitespace().next().unwrap_or("");
    match token_only.parse::<ProcessStatus>() {
        Ok(ProcessStatus::Initializing | ProcessStatus::Running) => Ok(true),
        _ => Ok(false),
    }
}
