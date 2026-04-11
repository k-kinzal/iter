//! Read/write of `<dir>/bootstrap_token`, the anti-accidental-adoption file
//! used by the parent → child handshake on `iter run --detach`.
//!
//! See plan section D for the role of this token. It is **not** a security
//! boundary against another OS user — `~/.iter` itself already restricts
//! to the owning user via mode `0o700`.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use crate::process::error::TokenCorruptKind;
use crate::process::id::BootstrapToken;
use crate::process::paths::{FILE_MODE, names};

/// Surface returned by [`read`].
#[derive(Debug)]
pub(crate) enum TokenReadError {
    /// Token file does not exist (already adopted, or never created).
    NotFound,
    /// File present but its body did not pass `validate` checks.
    Corrupt(TokenCorruptKind),
    /// Other I/O failure.
    Io(io::Error),
}

/// Read `<dirfd>/bootstrap_token` and parse it into a [`BootstrapToken`].
///
/// Validation rules (per plan D4):
/// - Length must be exactly 32 hex bytes (16 bytes encoded).
/// - Lower-case ASCII hex only.
#[cfg(unix)]
pub(crate) fn read(dirfd: BorrowedFd<'_>) -> Result<BootstrapToken, TokenReadError> {
    let name = std::ffi::CString::new(names::BOOTSTRAP_TOKEN).expect("static name has no NUL");
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        return Err(if err.raw_os_error() == Some(libc::ENOENT) {
            TokenReadError::NotFound
        } else {
            TokenReadError::Io(err)
        });
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(owned);
    let mut buf = String::with_capacity(32);
    if let Err(e) = file.read_to_string(&mut buf) {
        return Err(TokenReadError::Io(e));
    }
    let trimmed = buf.trim_end_matches(['\n', '\r']);
    parse(trimmed).map_err(TokenReadError::Corrupt)
}

#[cfg(not(unix))]
pub fn read(_dirfd: BorrowedFd<'_>) -> Result<BootstrapToken, TokenReadError> {
    Err(TokenReadError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "bootstrap_token::read is unix-only",
    )))
}

fn parse(hex: &str) -> Result<BootstrapToken, TokenCorruptKind> {
    if hex.len() != 32 {
        return Err(TokenCorruptKind::WrongLength { actual: hex.len() });
    }
    if hex.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(TokenCorruptKind::UpperCase);
    }
    if !hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err(TokenCorruptKind::NonHex(hex.to_owned()));
    }
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        let pair = &hex[i * 2..i * 2 + 2];
        *b = u8::from_str_radix(pair, 16).map_err(|_| TokenCorruptKind::NonHex(hex.to_owned()))?;
    }
    Ok(BootstrapToken::from_bytes(bytes))
}

/// Create `<dirfd>/bootstrap_token` exclusively (`O_CREAT|O_EXCL|0600`) and
/// write the token's hex body. Existing file → `EEXIST` propagates.
#[cfg(unix)]
pub(crate) fn write_excl(dirfd: BorrowedFd<'_>, token: &BootstrapToken) -> io::Result<()> {
    let name = std::ffi::CString::new(names::BOOTSTRAP_TOKEN).expect("static name has no NUL");
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            FILE_MODE,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(owned);
    file.write_all(token.to_hex().as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub fn write_excl(_dirfd: BorrowedFd<'_>, _token: &BootstrapToken) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bootstrap_token::write_excl is unix-only",
    ))
}

/// Best-effort delete of `<dirfd>/bootstrap_token`. ENOENT is **not** an
/// error (the file may already have been deleted).
#[cfg(unix)]
pub(crate) fn delete(dirfd: BorrowedFd<'_>) -> io::Result<()> {
    let name = std::ffi::CString::new(names::BOOTSTRAP_TOKEN).expect("static name has no NUL");
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

#[cfg(not(unix))]
pub fn delete(_dirfd: BorrowedFd<'_>) -> io::Result<()> {
    Ok(())
}

/// Predicate used by tests to assert the bootstrap token has (or has
/// not) been written. Production callers transition from `delete` /
/// `read` directly; the file-system probe is only useful for assertions.
#[cfg(all(unix, test))]
pub(crate) fn exists(dirfd: BorrowedFd<'_>) -> bool {
    let name = std::ffi::CString::new(names::BOOTSTRAP_TOKEN).expect("static name has no NUL");
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe {
        libc::fstatat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            &raw mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    r == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::id::ProcessId;
    use crate::process::paths::ProcPaths;
    use tempfile::TempDir;

    #[test]
    fn parse_accepts_lower_hex_32() {
        let hex = "00112233445566778899aabbccddeeff";
        let t = parse(hex).expect("ok");
        assert_eq!(t.to_hex(), hex);
    }

    #[test]
    fn parse_rejects_uppercase() {
        let res = parse("00112233445566778899AABBCCDDEEFF");
        assert!(matches!(res, Err(TokenCorruptKind::UpperCase)));
    }

    #[test]
    fn parse_rejects_wrong_length() {
        assert!(matches!(
            parse("abcd"),
            Err(TokenCorruptKind::WrongLength { actual: 4 })
        ));
    }

    #[test]
    fn parse_rejects_non_hex() {
        assert!(matches!(
            parse("ggggggggggggggggggggggggggggggg!"),
            Err(TokenCorruptKind::NonHex(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn write_then_read_round_trip() {
        let tmp = TempDir::new().unwrap();
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).unwrap();
        let token = BootstrapToken::generate();
        write_excl(paths.dirfd(), &token).expect("write");
        let parsed = read(paths.dirfd()).expect("read");
        assert_eq!(parsed.to_hex(), token.to_hex());
    }

    #[cfg(unix)]
    #[test]
    fn read_returns_not_found_when_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).unwrap();
        match read(paths.dirfd()) {
            Err(TokenReadError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn second_write_returns_eexist() {
        let tmp = TempDir::new().unwrap();
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).unwrap();
        let token = BootstrapToken::generate();
        write_excl(paths.dirfd(), &token).expect("first");
        let err = write_excl(paths.dirfd(), &token).expect_err("second");
        assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
    }

    #[cfg(unix)]
    #[test]
    fn delete_after_write_makes_read_not_found() {
        let tmp = TempDir::new().unwrap();
        let paths = ProcPaths::create_for_new_id(tmp.path(), ProcessId::generate()).unwrap();
        let token = BootstrapToken::generate();
        write_excl(paths.dirfd(), &token).unwrap();
        assert!(exists(paths.dirfd()));
        delete(paths.dirfd()).unwrap();
        assert!(!exists(paths.dirfd()));
        match read(paths.dirfd()) {
            Err(TokenReadError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
