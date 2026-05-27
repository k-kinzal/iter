//! Status-file body primitives: byte-level read, in-place rewrite, fsync,
//! and the rollback `Failed` writer.
//!
//! These are the only helpers that touch the on-disk body of the status
//! file directly; every higher-level operation in
//! [`super::ProcessStatusFile`] composes them inside a `flock`-protected
//! critical section.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::process::error::SecondaryStatusWriteResult;
use crate::process::status::{CorruptStatusError, CorruptStatusKind, ProcessStatus};

/// `seek(0) + set_len(0) + write_all` — required to ensure no partial-byte
/// residue from a previous write survives a rollback (rev12 invariant 3).
pub(super) fn write_status_in_place(file: &mut File, to: ProcessStatus) -> io::Result<()> {
    let body = format!("{}\n", to.as_serde_str());
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(body.as_bytes())?;
    Ok(())
}

/// Read and parse the status body. Decoding failures are returned as a typed
/// [`CorruptStatusError`] so the caller can decide whether to surface them or
/// reconcile to `Failed`.
pub(super) fn read_status(file: &mut File) -> Result<ProcessStatus, CorruptStatusError> {
    let mut buf = Vec::with_capacity(32);
    if let Err(e) = file.seek(SeekFrom::Start(0)) {
        return Err(CorruptStatusError {
            kind: CorruptStatusKind::TruncatedRead { read: 0 },
            raw_bytes: format!("seek failed: {e}").into_bytes(),
        });
    }
    if let Err(e) = file.read_to_end(&mut buf) {
        return Err(CorruptStatusError {
            kind: CorruptStatusKind::TruncatedRead { read: buf.len() },
            raw_bytes: format!("read failed: {e}").into_bytes(),
        });
    }
    if buf.is_empty() {
        return Err(CorruptStatusError {
            kind: CorruptStatusKind::EmptyBody,
            raw_bytes: buf,
        });
    }
    let Ok(raw) = std::str::from_utf8(&buf) else {
        return Err(CorruptStatusError {
            kind: CorruptStatusKind::UnknownToken(String::from_utf8_lossy(&buf).into_owned()),
            raw_bytes: buf,
        });
    };
    let trimmed = raw.trim_end_matches(['\n', '\r']);
    // Detect trailing garbage: anything after the canonical token.
    let token_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let token = &trimmed[..token_end];
    let after = &trimmed[token_end..];
    if !after.trim().is_empty() {
        return Err(CorruptStatusError {
            kind: CorruptStatusKind::TrailingGarbage,
            raw_bytes: buf,
        });
    }
    match token.parse::<ProcessStatus>() {
        Ok(s) => Ok(s),
        Err(_) => Err(CorruptStatusError {
            kind: CorruptStatusKind::UnknownToken(token.to_owned()),
            raw_bytes: buf,
        }),
    }
}

/// Rollback path: rewrite the body to `Failed` plus an `fsync`, swallowing
/// the underlying I/O results into a [`SecondaryStatusWriteResult`] so the
/// caller can report exactly which step survived.
pub(super) fn best_effort_mark_failed(file: &mut File) -> SecondaryStatusWriteResult {
    let write_result = write_status_in_place(file, ProcessStatus::Failed);
    let fsync_result = fsync_with_one_retry(file);
    SecondaryStatusWriteResult::from_write_and_fsync(write_result, fsync_result)
}

/// `fsync_data` with a single retry. Used as the durability fence after every
/// status write inside a flock critical section.
pub(super) fn fsync_with_one_retry(file: &File) -> io::Result<()> {
    match file.sync_data() {
        Ok(()) => Ok(()),
        Err(_) => file.sync_data(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use tempfile::TempDir;

    #[test]
    fn write_status_in_place_truncates_previous_body() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("status");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"running\n").unwrap();
        write_status_in_place(&mut file, ProcessStatus::Failed).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, "failed\n");
        assert!(body.len() < "running\n".len() + "failed\n".len());
    }

    #[test]
    fn read_status_returns_corrupt_for_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("status");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let err = read_status(&mut file).expect_err("empty");
        assert_eq!(err.kind, CorruptStatusKind::EmptyBody);
    }

    #[test]
    fn read_status_returns_corrupt_for_unknown_token() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("status");
        std::fs::write(&path, b"goofy\n").unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let err = read_status(&mut file).expect_err("unknown");
        assert!(matches!(err.kind, CorruptStatusKind::UnknownToken(_)));
    }
}
