//! Reading and classifying `<dir>/pid`.
//!
//! [`read`] walks the file with `O_NOFOLLOW`, applies the security checks
//! (regular-file, owner uid match, mode = 0o600, link count), and parses the
//! body into [`ProcessIdentity`]. The return type, [`PidFileState`], keeps
//! lifecycle evidence (`NotFound` / Corrupt) distinct from environmental
//! errors (`PermissionDenied` / `SecurityViolation` / `IoTransient` / `IoFatal`) so
//! `reconcile_under_lock` can route accordingly.

use std::io;
#[cfg(unix)]
use std::io::Read;
#[cfg(not(unix))]
use std::os::fd::BorrowedFd;
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg(unix)]
use crate::process::paths::names;

use super::cleanup::pid_residue_predicate;
use super::identity::{ParseIdentityError, ProcessIdentity};

/// Hard ceiling on `<dir>/pid` body size. Canonical lines fit comfortably
/// in well under 100 bytes; this leaves headroom while bounding the
/// allocation we perform in response to a faulty or hostile file.
#[cfg(unix)]
const MAX_PID_FILE_BYTES: usize = 256;

/// State of `<dir>/pid` as observed by [`read`].
///
/// Distinguishes lifecycle evidence (`NotFound` / Corrupt) from environmental
/// errors (`PermissionDenied` / `SecurityViolation` / `IoTransient` / `IoFatal`) so
/// `reconcile_under_lock` can treat them differently.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum PidFileState {
    /// File parsed cleanly.
    Found(ProcessIdentity),
    /// File does not exist (`ENOENT`). Lifecycle evidence: child has not yet
    /// written it, or it was deleted after termination.
    NotFound,
    /// File exists but its body could not be parsed.
    Corrupt(CorruptKind),
    /// `EACCES` / `EPERM`. Environmental — does not change status.
    PermissionDenied,
    /// Ownership / mode / hardlink / symlink anomaly.
    SecurityViolation(SecurityKind),
    /// `EINTR` / `EAGAIN` — transient, retryable. Environmental.
    IoTransient(io::Error),
    /// `EIO` / `ESTALE` / `ENODEV` etc. Environmental.
    IoFatal(io::Error),
}

/// Why the pid-file body did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum CorruptKind {
    /// Zero bytes / only whitespace.
    EmptyBody,
    /// Read returned fewer bytes than expected before EOF.
    Truncated {
        /// Number of bytes returned before EOF.
        read: usize,
    },
    /// OS tag was neither `linux` nor `macos`.
    UnknownPrefix(String),
    /// pid component did not parse.
    InvalidPid(String),
    /// start-time component did not parse.
    InvalidStartTime(String),
    /// Linux boot-id was malformed.
    InvalidBootId(String),
    /// Body had bytes after the canonical token.
    TrailingGarbage,
    /// `nlink == 2` and `.pid.tmp` is the second link (linkat residue).
    /// Lifecycle evidence — `reconcile_under_lock` will Failed + cleanup.
    PartialAdoptionResidue {
        /// Link count observed.
        nlink: u64,
    },
}

/// Why a security check rejected the pid file (or its parent directory).
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum SecurityKind {
    /// File was a symbolic link rather than a regular file.
    Symlink,
    /// Owner uid did not match `geteuid()`.
    OwnerMismatch {
        /// Expected uid (current effective uid).
        expected_uid: u32,
        /// Actual uid stored in the inode.
        actual_uid: u32,
    },
    /// File mode had bits set in the group/other bytes (i.e. not `0o600`).
    BadMode {
        /// Actual mode bits.
        actual: u32,
    },
    /// File was not a regular file.
    NotRegularFile {
        /// Observed file type.
        kind: FileTypeName,
    },
    /// `nlink == 2` without inode-matching `.pid.tmp`, or `nlink ≥ 3`.
    UnexpectedHardlinks {
        /// Observed link count.
        nlink: u64,
    },
}

/// Human-readable file type for [`SecurityKind::NotRegularFile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum FileTypeName {
    /// Directory.
    Directory,
    /// FIFO (named pipe).
    Fifo,
    /// Unix-domain socket.
    Socket,
    /// Block device.
    BlockDevice,
    /// Character device.
    CharDevice,
    /// Anything else.
    Other,
}

/// Read `<dir>/pid` with full security checks.
#[cfg(unix)]
#[must_use]
pub(crate) fn read(dirfd: BorrowedFd<'_>) -> PidFileState {
    let dirfd_raw = dirfd.as_raw_fd();

    let Ok(pid_name) = std::ffi::CString::new(names::PID) else {
        return PidFileState::Corrupt(CorruptKind::EmptyBody);
    };
    // SAFETY: `dirfd_raw` is copied from a live borrowed directory fd and
    // `pid_name` is NUL-terminated and valid for the duration of `openat`.
    let fd = unsafe {
        libc::openat(
            dirfd_raw,
            pid_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return classify_open_errno(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by successful `openat` and has no other
    // Rust owner, so transferring ownership to `OwnedFd` is correct.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    // fstat for security + size + nlink
    let meta = match fstat_metadata(&fd) {
        Ok(m) => m,
        Err(io_err) => return PidFileState::IoFatal(io_err),
    };

    if let Some(violation) = check_security(&meta) {
        return PidFileState::SecurityViolation(violation);
    }

    // nlink check (rev13 / rev14)
    let nlink = meta.nlink();
    if nlink > 1 {
        if nlink == 2 && pid_residue_predicate(dirfd) {
            return PidFileState::Corrupt(CorruptKind::PartialAdoptionResidue { nlink });
        }
        return PidFileState::SecurityViolation(SecurityKind::UnexpectedHardlinks { nlink });
    }

    // Reject oversized bodies without reading them. A canonical pid line
    // is at most ~80 bytes (`<u32>:linux:<u64>:<32-byte boot_id>\n`); a
    // 256-byte ceiling leaves headroom while bounding the allocation we
    // perform in response to an attacker- or fault-grown pid file.
    if meta.len() > MAX_PID_FILE_BYTES as u64 {
        return PidFileState::Corrupt(CorruptKind::TrailingGarbage);
    }

    // Read body — bounded by `take` as a defence in depth; the prior
    // `metadata` check already covers the steady-state path.
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(MAX_PID_FILE_BYTES));
    let file = std::fs::File::from(fd);
    if let Err(io_err) = file
        .take(MAX_PID_FILE_BYTES as u64 + 1)
        .read_to_end(&mut buf)
    {
        return PidFileState::IoTransient(io_err);
    }
    if buf.len() > MAX_PID_FILE_BYTES {
        return PidFileState::Corrupt(CorruptKind::TrailingGarbage);
    }

    if buf.is_empty() {
        return PidFileState::Corrupt(CorruptKind::EmptyBody);
    }

    let Ok(raw) = std::str::from_utf8(&buf) else {
        return PidFileState::Corrupt(CorruptKind::UnknownPrefix(
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    };

    parse_body(raw)
}

#[cfg(not(unix))]
pub(crate) fn read(_dirfd: BorrowedFd<'_>) -> PidFileState {
    PidFileState::IoFatal(io::Error::new(
        io::ErrorKind::Unsupported,
        "pid_file::read is unix-only",
    ))
}

/// Map a parsed body to either `Found(ProcessIdentity)` or `Corrupt(...)`.
#[cfg(unix)]
fn parse_body(raw: &str) -> PidFileState {
    match raw.parse::<ProcessIdentity>() {
        Ok(id) => PidFileState::Found(id),
        Err(e) => match e {
            ParseIdentityError::Empty => PidFileState::Corrupt(CorruptKind::EmptyBody),
            ParseIdentityError::MissingOs | ParseIdentityError::UnknownOs(_) => {
                PidFileState::Corrupt(CorruptKind::UnknownPrefix(raw.to_owned()))
            }
            ParseIdentityError::InvalidPid(s) => PidFileState::Corrupt(CorruptKind::InvalidPid(s)),
            ParseIdentityError::MissingStartTime | ParseIdentityError::InvalidStartTime(_) => {
                let s = match e {
                    ParseIdentityError::InvalidStartTime(s) => s,
                    _ => raw.to_owned(),
                };
                PidFileState::Corrupt(CorruptKind::InvalidStartTime(s))
            }
            ParseIdentityError::MissingBootId | ParseIdentityError::InvalidBootId(_) => {
                let s = match e {
                    ParseIdentityError::InvalidBootId(s) => s,
                    _ => String::new(),
                };
                PidFileState::Corrupt(CorruptKind::InvalidBootId(s))
            }
        },
    }
}

/// Validate file type, owner uid, and mode bits against the rev13/14 rules.
/// `nlink` is checked separately by the caller — it has lifecycle evidence
/// semantics (`PartialAdoptionResidue`) that don't fit a simple
/// `Option<SecurityKind>`.
#[cfg(unix)]
fn check_security(meta: &std::fs::Metadata) -> Option<SecurityKind> {
    if !meta.file_type().is_file() {
        let kind = file_type_name(meta);
        return Some(SecurityKind::NotRegularFile { kind });
    }

    let actual_uid = meta.uid();
    // SAFETY: `geteuid` reads process credentials and has no memory-safety
    // preconditions.
    let expected_uid = unsafe { libc::geteuid() };
    if actual_uid != expected_uid {
        return Some(SecurityKind::OwnerMismatch {
            expected_uid,
            actual_uid,
        });
    }

    let mode = meta.mode() & 0o777;
    // Exact match against `FILE_MODE` (0o600). The original `mode & 0o077`
    // mask let same-uid attackers grow a `0o400`-or-`0o700` pid file past
    // the security gate; only files written by the canonical publish path
    // (`openat` + `fchmod 0o600`) are accepted here.
    if mode != crate::process::paths::FILE_MODE {
        return Some(SecurityKind::BadMode { actual: mode });
    }

    None
}

#[cfg(unix)]
fn classify_open_errno(err: io::Error) -> PidFileState {
    let raw = err.raw_os_error();
    match raw {
        Some(libc::ENOENT) => PidFileState::NotFound,
        Some(libc::EACCES | libc::EPERM) => PidFileState::PermissionDenied,
        Some(libc::ELOOP) => PidFileState::SecurityViolation(SecurityKind::Symlink),
        Some(libc::EINTR | libc::EAGAIN) => PidFileState::IoTransient(err),
        _ => PidFileState::IoFatal(err),
    }
}

#[cfg(unix)]
fn fstat_metadata(fd: &OwnedFd) -> io::Result<std::fs::Metadata> {
    // Borrow the fd via `ManuallyDrop<File>` so we can call `metadata()`
    // without consuming the original `OwnedFd` and without paying for an
    // extra `fcntl(F_DUPFD_CLOEXEC)` syscall.
    use std::mem::ManuallyDrop;
    // SAFETY: `fd.as_raw_fd()` is a live descriptor owned by `fd`; wrapping
    // the temporary `File` in `ManuallyDrop` prevents it from closing the fd.
    let borrowed = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) });
    borrowed.metadata()
}

#[cfg(unix)]
fn file_type_name(meta: &std::fs::Metadata) -> FileTypeName {
    use std::os::unix::fs::FileTypeExt;
    let ft = meta.file_type();
    if ft.is_dir() {
        FileTypeName::Directory
    } else if ft.is_fifo() {
        FileTypeName::Fifo
    } else if ft.is_socket() {
        FileTypeName::Socket
    } else if ft.is_block_device() {
        FileTypeName::BlockDevice
    } else if ft.is_char_device() {
        FileTypeName::CharDevice
    } else {
        FileTypeName::Other
    }
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
    fn read_returns_found_after_publish() {
        use crate::process::id::ProcessId;
        use crate::process::paths::ProcPaths;
        use crate::process::pid_file::publish::write_atomic_at;
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        let id = linux_id();
        write_atomic_at(paths.dirfd(), &id).expect("publish");
        match read(paths.dirfd()) {
            PidFileState::Found(parsed) => assert_eq!(parsed, id),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_returns_not_found_for_empty_dir() {
        use crate::process::id::ProcessId;
        use crate::process::paths::ProcPaths;
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tmp");
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).expect("paths");
        match read(paths.dirfd()) {
            PidFileState::NotFound => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
