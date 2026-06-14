//! Atomic `linkat`-only publication of `<dir>/pid`.
//!
//! See module-level docs in [`super`] for why publication uses
//! `linkat(flag = 0)` rather than `renameat`. This file owns the typed
//! failure surface ([`PublishError`] / [`PublishStep`]) and the syscall
//! sequence itself.

use std::fmt;
use std::io;
#[cfg(not(unix))]
use std::os::fd::BorrowedFd;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

#[cfg(unix)]
use crate::process::paths::{FILE_MODE, names};

use super::identity::ProcessIdentity;
#[cfg(unix)]
use super::syscall::{fsync_dirfd, fsync_fd, unlinkat_name, write_all};

/// Failure surface of the pid-file publication primitive.
///
/// The two `EEXIST` shapes are split because they have different recovery
/// semantics:
///
/// - [`PublishError::PidTmpResidue`] — `.pid.tmp` already exists. Indicates
///   that a previous startup/adoption crashed between `openat(.pid.tmp,
///   O_CREAT|O_EXCL)` and `unlinkat(.pid.tmp)`. Janitor + grace-period
///   reconciliation cleans this up.
/// - [`PublishError::PidAlreadyPresent`] — `linkat(.pid.tmp → pid)`
///   returned `EEXIST`. `pid` is supposed to be absent during
///   `Initializing`; encountering it here means a prior session linked
///   the file but crashed before clearing the directory. Requires
///   `iter rm` (or `refresh_status` driven cleanup) to recover.
/// - [`PublishError::Io`] — any other I/O failure, tagged with the step
///   that observed it so callers can surface "where" without rebuilding
///   the syscall sequence from a stack trace.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum PublishError {
    /// `openat(.pid.tmp, O_CREAT|O_EXCL)` returned `EEXIST`.
    PidTmpResidue {
        /// Underlying `io::Error` carrying the errno.
        source: io::Error,
    },
    /// `linkat(.pid.tmp → pid, flag=0)` returned `EEXIST`.
    PidAlreadyPresent {
        /// Underlying `io::Error` carrying the errno.
        source: io::Error,
    },
    /// Any other I/O failure observed during one of the publication steps.
    Io {
        /// Underlying `io::Error` carrying the errno.
        source: io::Error,
        /// Which step observed the failure.
        step: PublishStep,
    },
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PublishError::PidTmpResidue { source } => {
                write!(f, "pid_file: stale `.pid.tmp` already present: {source}")
            }
            PublishError::PidAlreadyPresent { source } => {
                write!(
                    f,
                    "pid_file: `pid` already present (crash recovery needed): {source}"
                )
            }
            PublishError::Io { source, step } => {
                write!(f, "pid_file: I/O failure during {step:?}: {source}")
            }
        }
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PublishError::PidTmpResidue { source }
            | PublishError::PidAlreadyPresent { source }
            | PublishError::Io { source, .. } => Some(source),
        }
    }
}

/// Position inside the pid-file publication sequence that observed an I/O error.
///
/// Sequence (in order):
/// 1. `openat(dirfd, ".pid.tmp", O_CREAT|O_EXCL|O_WRONLY|O_CLOEXEC|O_NOFOLLOW, 0o600)`
/// 2. `fchmod(tmp_fd, 0o600)` — defeats process umask so the file ends up
///    at exactly `FILE_MODE`.
/// 3. `write_all(identity_bytes)`
/// 4. `fsync(tmp_fd)`
/// 5. `linkat(dirfd, ".pid.tmp", dirfd, "pid", 0)`
/// 6. `unlinkat(dirfd, ".pid.tmp", 0)`
/// 7. `fsync(dirfd)`
///
/// `Linkat` here covers *non-EEXIST* I/O failures only; the `EEXIST` shape
/// has its own [`PublishError::PidAlreadyPresent`] variant. Likewise `OpenTmp`
/// covers only non-`EEXIST` failures (the `EEXIST` shape is
/// [`PublishError::PidTmpResidue`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum PublishStep {
    /// `openat` of `.pid.tmp` (non-EEXIST failure).
    OpenTmp,
    /// `fchmod(tmp_fd, FILE_MODE)` to defeat the process umask.
    FchmodTmp,
    /// `write_all` of the identity bytes into `.pid.tmp`.
    WriteTmp,
    /// `fsync` of the tmp fd before linking.
    FsyncTmp,
    /// `linkat(.pid.tmp → pid, flag=0)` (non-EEXIST failure).
    Linkat,
    /// `unlinkat(.pid.tmp)` after a successful link. A failure here leaves
    /// `pid` durable but with `nlink == 2`; later cleanup paths (and the
    /// `pid_residue_predicate` helper) handle this.
    UnlinkTmp,
    /// `fsync(dirfd)` to make the directory entry durable.
    FsyncDir,
}

/// Atomically publish `<dir>/pid` containing `identity`.
/// # Errors
///
/// Returns an error if the operation fails.
///
/// Unix-only. The full sequence is documented at [`PublishStep`]. The dirfd
/// must have been opened with `O_DIRECTORY|O_CLOEXEC|O_RDONLY`.
///
/// # Panics
///
/// Panics if the static names `.pid.tmp` or `pid` cannot be converted to a
/// `CString`, which can only happen if either contains an interior NUL — a
/// compile-time impossibility.
#[cfg(unix)]
pub(crate) fn write_atomic_at(
    dirfd: BorrowedFd<'_>,
    identity: &ProcessIdentity,
) -> Result<(), PublishError> {
    let dirfd_raw = dirfd.as_raw_fd();
    let bytes = identity.to_pid_line();

    // 1. openat(.pid.tmp, O_CREAT|O_EXCL|O_WRONLY|O_CLOEXEC|O_NOFOLLOW, 0o600)
    // SAFETY: `dirfd_raw` is copied from a live borrowed directory fd; the
    // CString is NUL-terminated and `FILE_MODE` is a valid creation mode.
    let tmp_fd = unsafe {
        let pid_tmp = std::ffi::CString::new(names::PID_TMP).expect("static name has no NUL");
        let flags =
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        libc::openat(dirfd_raw, pid_tmp.as_ptr(), flags, FILE_MODE)
    };
    if tmp_fd < 0 {
        let err = io::Error::last_os_error();
        return Err(if err.raw_os_error() == Some(libc::EEXIST) {
            PublishError::PidTmpResidue { source: err }
        } else {
            PublishError::Io {
                source: err,
                step: PublishStep::OpenTmp,
            }
        });
    }
    // SAFETY: `tmp_fd` was just returned by successful `openat` and has no
    // other Rust owner, so transferring ownership to `OwnedFd` is correct.
    let tmp_fd = unsafe { OwnedFd::from_raw_fd(tmp_fd) };

    // 1b. fchmod(0o600) to defeat the process umask. Without this an
    // 0o077 umask leaves the file at 0o600, but tighter umasks (0o277,
    // 0o377, …) silently strip owner bits, and `read::check_security`
    // now requires an exact match against `FILE_MODE`.
    // SAFETY: `tmp_fd` is a live owned file descriptor; the mode value is
    // converted to `mode_t` and `fchmod` has no pointer preconditions.
    let chmod_ret = unsafe {
        libc::fchmod(
            tmp_fd.as_raw_fd(),
            libc::mode_t::try_from(FILE_MODE).expect("FILE_MODE fits in mode_t"),
        )
    };
    if chmod_ret != 0 {
        let err = io::Error::last_os_error();
        drop(unlinkat_name(dirfd, names::PID_TMP));
        return Err(PublishError::Io {
            source: err,
            step: PublishStep::FchmodTmp,
        });
    }

    // 2. write_all
    write_all(&tmp_fd, bytes.as_bytes()).map_err(|source| PublishError::Io {
        source,
        step: PublishStep::WriteTmp,
    })?;

    // 3. fsync(tmp_fd)
    fsync_fd(&tmp_fd).map_err(|source| PublishError::Io {
        source,
        step: PublishStep::FsyncTmp,
    })?;

    // 4. linkat(.pid.tmp → pid, flag=0) — create-fail-if-exists.
    // SAFETY: `dirfd_raw` is copied from a live borrowed directory fd, and
    // both CStrings are NUL-terminated and live for the syscall.
    let link_ret = unsafe {
        let from = std::ffi::CString::new(names::PID_TMP).expect("static name");
        let to = std::ffi::CString::new(names::PID).expect("static name");
        libc::linkat(dirfd_raw, from.as_ptr(), dirfd_raw, to.as_ptr(), 0)
    };
    if link_ret != 0 {
        let err = io::Error::last_os_error();
        // Best-effort cleanup of `.pid.tmp` so the next call does not see
        // a stale residue. We deliberately ignore the cleanup result —
        // the primary error is the link failure.
        drop(unlinkat_name(dirfd, names::PID_TMP));
        return Err(if err.raw_os_error() == Some(libc::EEXIST) {
            PublishError::PidAlreadyPresent { source: err }
        } else {
            PublishError::Io {
                source: err,
                step: PublishStep::Linkat,
            }
        });
    }

    // 5. unlinkat(.pid.tmp)
    if let Err(err) = unlinkat_name(dirfd, names::PID_TMP) {
        return Err(PublishError::Io {
            source: err,
            step: PublishStep::UnlinkTmp,
        });
    }

    // 6. fsync(dirfd)
    fsync_dirfd(dirfd).map_err(|source| PublishError::Io {
        source,
        step: PublishStep::FsyncDir,
    })?;

    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_atomic_at(
    _dirfd: BorrowedFd<'_>,
    _identity: &ProcessIdentity,
) -> Result<(), PublishError> {
    Err(PublishError::Io {
        source: io::Error::new(io::ErrorKind::Unsupported, "windows not supported"),
        step: PublishStep::OpenTmp,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    fn linux_id() -> ProcessIdentity {
        use crate::process::id::Pid;
        use crate::process::proc_info::ProcessStartTime;
        ProcessIdentity {
            pid: Pid::new(1234),
            start_time: ProcessStartTime::LinuxClockTicks(98765),
            linux_boot_id: Some("abcdef0123456789-abcd".into()),
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_at_creates_pid_file_and_removes_tmp() {
        use crate::process::id::ProcessId;
        use crate::process::paths::ProcPaths;
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        let id = linux_id();
        write_atomic_at(paths.dirfd(), &id).expect("publish");

        let pid_path = paths.dir().join(names::PID);
        assert!(pid_path.exists());
        let body = std::fs::read_to_string(&pid_path).expect("read body");
        assert_eq!(body, id.to_pid_line());

        let tmp_path = paths.dir().join(names::PID_TMP);
        assert!(!tmp_path.exists(), "tmp should have been unlinked");
    }

    #[cfg(unix)]
    #[test]
    fn second_publish_returns_pid_already_present() {
        use crate::process::id::ProcessId;
        use crate::process::paths::ProcPaths;
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        let id = linux_id();
        write_atomic_at(paths.dirfd(), &id).expect("first publish");
        let err = write_atomic_at(paths.dirfd(), &id).expect_err("second must fail");
        assert!(
            matches!(err, PublishError::PidAlreadyPresent { .. }),
            "got {err:?}"
        );
    }
}
