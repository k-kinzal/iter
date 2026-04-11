//! Best-effort sweep of stale `.<name>.<32hex>.tmp` files.
//!
//! Runs at the start of every [`acquire`](super::acquire::acquire) call.
//! Only files that survive [`super::name::parse_tmp_name`]
//! (i.e. were definitely emitted by us) and are older than [`TMP_GRACE_SECS`]
//! are unlinked; everything else is left in place. Failures are swallowed —
//! the janitor must not mask the caller's primary error.

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::path::Path;

#[cfg(unix)]
use super::name::parse_tmp_name;
#[cfg(unix)]
use super::syscall::{best_effort_unlink, fstatat_nofollow, lock_age};

/// Tmp files older than this with the canonical `.<name>.<32hex>.tmp` shape
/// are unlinked at the start of every `acquire` call.
const TMP_GRACE_SECS: u64 = 5 * 60;

#[cfg(unix)]
pub(super) fn janitor_sweep(locks_dirfd: BorrowedFd<'_>, locks_dir_path: &Path) {
    let Ok(entries) = std::fs::read_dir(locks_dir_path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if parse_tmp_name(&name).is_none() {
            continue;
        }
        let Ok(cname) = CString::new(name.as_bytes()) else {
            continue;
        };
        let Ok(st) = fstatat_nofollow(locks_dirfd.as_raw_fd(), &cname) else {
            continue;
        };
        if (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
            continue;
        }
        if lock_age(&st).as_secs() < TMP_GRACE_SECS {
            continue;
        }
        best_effort_unlink(locks_dirfd, &cname);
    }
}

#[cfg(not(unix))]
pub(super) fn janitor_sweep(_locks_dirfd: BorrowedFd<'_>, _locks_dir_path: &Path) {}
