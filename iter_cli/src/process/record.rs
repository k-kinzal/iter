//! `ProcessRecord` — pure read view over `~/.iter/proc/<id>/`.
//!
//! `ProcessRecord` is a side-effect-free handle: every accessor opens the
//! relevant file lazily on demand. It owns no flock, never mutates any side
//! file, and is safe to construct from any thread (`Send + Sync`). Callers
//! that need to *change* state (`stop`, `kill`, `refresh_status`,
//! `remove`) go through [`ProcessHandle`](crate::process::handle::ProcessHandle).
//!
//! The record holds an `Arc<ProcPaths>` so that pid-file security checks and
//! other dirfd-based reads stay TOCTOU-safe.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::process::error::ProcessError;

type ProcessFallible<T> = Result<T, ProcessError>;
use crate::process::id::ProcessId;
use crate::process::metadata::ProcessMetadata;
use crate::process::paths::names::LOG_NDJSON;
use crate::process::paths::{ProcPaths, names, proc_root_default};
use crate::process::pid_file::{self, PidFileState};
use crate::process::status::ProcessStatus;
use iter_core::log::NdjsonReader;

/// Pure read view over a single proc directory.
#[derive(Debug, Clone)]
pub(crate) struct ProcessRecord {
    paths: Arc<ProcPaths>,
}

impl ProcessRecord {
    /// Wrap an existing `Arc<ProcPaths>`. Construction succeeds even if the
    /// directory is partially populated (e.g. mid-bootstrap); the individual
    /// accessors fail with appropriate errors.
    #[must_use]
    pub(crate) fn new(paths: Arc<ProcPaths>) -> Self {
        Self { paths }
    }

    /// Open `<root>/<id>/` and wrap it. The directory must already exist;
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// the dirfd is opened with `O_DIRECTORY|O_CLOEXEC|O_RDONLY`.
    pub(crate) fn open(root: &Path, id: ProcessId) -> ProcessFallible<Self> {
        let paths = ProcPaths::open_existing(root, id)?;
        Ok(Self::new(paths))
    }

    /// Open from an explicit directory path (used by `Registry::list`).
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// The directory's basename must be a valid `ProcessId`.
    pub(crate) fn from_dir(dir: &Path) -> ProcessFallible<Self> {
        let id = parse_id_from_dir(dir)?;
        let parent = dir.parent().ok_or_else(|| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("proc dir has no parent: {}", dir.display()),
            ))
        })?;
        let paths = ProcPaths::open_existing(parent, id)?;
        Ok(Self::new(paths))
    }

    /// `ProcessId` of this record.
    #[must_use]
    pub(crate) fn id(&self) -> ProcessId {
        self.paths.id()
    }

    /// Absolute path of `<root>/<id>/`.
    #[must_use]
    pub(crate) fn dir(&self) -> &Path {
        self.paths.dir()
    }

    /// Borrow the underlying `Arc<ProcPaths>` (used by `Handle`).
    #[must_use]
    pub(crate) fn paths(&self) -> &Arc<ProcPaths> {
        &self.paths
    }

    /// Read `name`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub(crate) fn name(&self) -> ProcessFallible<String> {
        read_trimmed(&self.paths.join(names::NAME))
    }

    /// Read `iterfile` and return it as a `PathBuf`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub(crate) fn iterfile(&self) -> ProcessFallible<PathBuf> {
        Ok(PathBuf::from(read_trimmed(
            &self.paths.join(names::ITERFILE),
        )?))
    }

    /// Read `subcommand`.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub(crate) fn subcommand(&self) -> ProcessFallible<String> {
        read_trimmed(&self.paths.join(names::SUBCOMMAND))
    }

    /// Read `started_at` and parse as RFC 3339.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub(crate) fn started_at(&self) -> ProcessFallible<DateTime<Utc>> {
        let raw = read_trimmed(&self.paths.join(names::STARTED_AT))?;
        DateTime::parse_from_rfc3339(&raw)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| {
                ProcessError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("started_at: {e}"),
                ))
            })
    }

    /// Deserialise `meta.json`. Validates that `meta.id` matches the
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// directory's id and returns `ProcessError::MetadataIdMismatch` otherwise.
    pub(crate) fn metadata(&self) -> ProcessFallible<ProcessMetadata> {
        let path = self.paths.join(names::META);
        let bytes = std::fs::read(&path).map_err(ProcessError::Io)?;
        let meta: ProcessMetadata =
            serde_json::from_slice(&bytes).map_err(ProcessError::JsonRead)?;
        if meta.id != self.id() {
            return Err(ProcessError::MetadataIdMismatch {
                expected: self.id(),
                found: meta.id,
            });
        }
        Ok(meta)
    }

    /// Read the persisted `status` token. **One-shot read without flock** —
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    /// the result reflects whatever was last written. For a flock-protected
    /// reconciliation use [`ProcessHandle::refresh_status`].
    ///
    /// [`ProcessHandle::refresh_status`]: crate::process::handle::ProcessHandle::refresh_status
    pub(crate) fn read_status_token(&self) -> ProcessFallible<ProcessStatus> {
        let raw = read_trimmed(&self.paths.join(names::STATUS))?;
        raw.parse::<ProcessStatus>().map_err(|e| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("status: {e}"),
            ))
        })
    }

    /// Same as [`Self::read_status_token`] but returns `Ok(None)` for the
    /// **single benign race** in which the writer is mid-rewrite of the
    /// status body.
    ///
    /// `status_file::body::write_status_in_place` performs `set_len(0)`
    /// followed by `write_all` as two syscalls under `flock(LOCK_EX)`,
    /// while this read is unflocked. A reader that lands between the two
    /// syscalls observes a zero-byte file. This helper distinguishes that
    /// transient state from genuine corruption (non-UTF8 bytes, unknown
    /// token) — the latter still surface as `Err(InvalidData)` so callers
    /// don't silently lose visibility of broken on-disk state. `ENOENT`
    /// is propagated as `Err(NotFound)` so the caller decides whether
    /// the missing file is a race or a real failure.
    ///
    /// Listing-style consumers (compose discovery, `iter ps`) should use
    /// this helper. Anything that needs the strict semantics of "give me
    /// the current token or fail" should keep using
    /// [`Self::read_status_token`].
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] for I/O failures other than the
    /// transient empty-body window, including non-UTF8 file contents and
    /// unrecognised tokens.
    pub(crate) fn read_status_token_or_in_transition(
        &self,
    ) -> ProcessFallible<Option<ProcessStatus>> {
        let path = self.paths.join(names::STATUS);
        let bytes = std::fs::read(&path).map_err(ProcessError::Io)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            ))
        })?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        trimmed.parse::<ProcessStatus>().map(Some).map_err(|e| {
            ProcessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("status: {e}"),
            ))
        })
    }

    /// Open the pid file via the dirfd-based reader (security checks +
    /// hardlink residue classification per rev13 §C3).
    #[must_use]
    pub(crate) fn pid_identity(&self) -> PidFileState {
        pid_file::read(self.paths.dirfd())
    }

    /// Tail the per-process `log.ndjson` stream.
    ///
    /// Each yielded record is a [`LogEntry`](iter_core::log::LogEntry)
    /// carrying a UTC timestamp, originating stream
    /// ([`LogStream::Stdout`](iter_core::log::LogStream::Stdout) or
    /// [`LogStream::Stderr`](iter_core::log::LogStream::Stderr)), and the
    /// line text (without the trailing newline). The reader supports
    /// `tail = Some(N)` for cap-the-initial-preload semantics and
    /// `follow = true` for `tail -f`-style polling.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub(crate) fn tail_log_ndjson(
        &self,
        follow: bool,
        tail: Option<usize>,
    ) -> ProcessFallible<NdjsonReader> {
        let path = self.paths.dir().join(LOG_NDJSON);
        NdjsonReader::open(&path, follow, tail).map_err(|e| match e {
            iter_core::log::NdjsonReadError::Io(io) => ProcessError::Io(io),
            iter_core::log::NdjsonReadError::Json(j) => ProcessError::JsonRead(j),
        })
    }
}

/// Read a file and trim trailing whitespace (typically a single `\n`).
pub(crate) fn read_trimmed(path: &Path) -> ProcessFallible<String> {
    let bytes = std::fs::read(path).map_err(ProcessError::Io)?;
    let text = String::from_utf8(bytes).map_err(|e| {
        ProcessError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        ))
    })?;
    Ok(text.trim().to_owned())
}

fn parse_id_from_dir(dir: &Path) -> ProcessFallible<ProcessId> {
    let name = dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        ProcessError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("proc dir has no name: {}", dir.display()),
        ))
    })?;
    name.parse::<ProcessId>().map_err(|e| {
        ProcessError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("proc dir name is not a valid ProcessId ({name}): {e}"),
        ))
    })
}

/// Convenience: list every existing record under the default proc root.
/// # Errors
///
/// Returns an error if the operation fails.
///
/// Skips entries whose names do not parse as a `ProcessId` (this includes
/// `.locks/`). Errors from individual entries are surfaced to the caller —
/// `iter ps` prefers to fail loudly rather than silently hide a half-broken
/// record.
pub(crate) fn list_default() -> ProcessFallible<Vec<ProcessRecord>> {
    let root = proc_root_default()?;
    list_under(&root)
}

/// Same as [`list_default`] but with an explicit `root` (used by tests).
/// # Errors
///
/// Returns an error if the operation fails.
pub(crate) fn list_under(root: &Path) -> ProcessFallible<Vec<ProcessRecord>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(ProcessError::Io(e)),
    };
    for entry in entries {
        let entry = entry.map_err(ProcessError::Io)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip `.locks/` and any other dot-directories.
        if name.starts_with('.') {
            continue;
        }
        let Ok(id) = name.parse::<ProcessId>() else {
            continue;
        };
        out.push(ProcessRecord::open(root, id)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_record(root: &Path) -> ProcessRecord {
        let id = ProcessId::generate();
        let id_dir = root.join(id.to_string());
        std::fs::create_dir_all(&id_dir).expect("create dir");
        ProcessRecord::open(root, id).expect("open")
    }

    #[test]
    fn from_dir_parses_id_from_directory_name() {
        let tmp = TempDir::new().expect("tmp");
        let id = ProcessId::generate();
        let dir = tmp.path().join(id.to_string());
        std::fs::create_dir_all(&dir).expect("mkdir");
        let rec = ProcessRecord::from_dir(&dir).expect("from_dir");
        assert_eq!(rec.id(), id);
        assert_eq!(rec.dir(), dir);
    }

    #[test]
    fn from_dir_rejects_non_ulid_name() {
        let tmp = TempDir::new().expect("tmp");
        let dir = tmp.path().join("not-a-ulid");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = ProcessRecord::from_dir(&dir).expect_err("must fail");
        match err {
            ProcessError::Io(e) => assert!(e.to_string().contains("not a valid ProcessId")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn name_subcommand_iterfile_round_trip() {
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        std::fs::write(rec.dir().join(names::NAME), "alpha\n").expect("write name");
        std::fs::write(rec.dir().join(names::SUBCOMMAND), "run\n").expect("write subcommand");
        std::fs::write(rec.dir().join(names::ITERFILE), "/tmp/Iterfile\n").expect("write iterfile");
        assert_eq!(rec.name().expect("name"), "alpha");
        assert_eq!(rec.subcommand().expect("subcommand"), "run");
        assert_eq!(
            rec.iterfile().expect("iterfile"),
            PathBuf::from("/tmp/Iterfile")
        );
    }

    #[test]
    fn started_at_round_trips_rfc3339() {
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        let when = Utc::now();
        std::fs::write(rec.dir().join(names::STARTED_AT), when.to_rfc3339()).expect("write");
        let read_back = rec.started_at().expect("parse");
        assert_eq!(read_back.timestamp_millis(), when.timestamp_millis());
    }

    #[test]
    fn read_status_token_round_trip() {
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        for status in [
            ProcessStatus::Initializing,
            ProcessStatus::Running,
            ProcessStatus::Stopped,
            ProcessStatus::Failed,
            ProcessStatus::Killed,
        ] {
            let body = format!("{}\n", status.as_serde_str());
            std::fs::write(rec.dir().join(names::STATUS), body).expect("write");
            assert_eq!(rec.read_status_token().expect("parse"), status);
        }
    }

    #[test]
    fn read_status_token_unknown_errors() {
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        std::fs::write(rec.dir().join(names::STATUS), "bogus\n").expect("write");
        let err = rec.read_status_token().expect_err("must fail");
        match err {
            ProcessError::Io(e) => assert!(e.to_string().contains("status:")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn metadata_round_trips_through_json() {
        use std::collections::BTreeMap;
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        let mut labels = BTreeMap::new();
        labels.insert("iter.compose.project".into(), "demo".into());
        let meta = ProcessMetadata {
            id: rec.id(),
            name: "demo".into(),
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into(), "--debug".into()],
            env: vec![("FOO".into(), "bar".into())],
            debug: true,
            parent_id: None,
            labels,
        };
        let bytes = serde_json::to_vec(&meta).expect("serialize");
        std::fs::write(rec.dir().join(names::META), bytes).expect("write");
        let read_back = rec.metadata().expect("read");
        assert_eq!(read_back, meta);
    }

    #[test]
    fn metadata_rejects_id_mismatch() {
        use std::collections::BTreeMap;
        let tmp = TempDir::new().expect("tmp");
        let rec = make_record(tmp.path());
        let other_id = ProcessId::generate();
        let meta = ProcessMetadata {
            id: other_id,
            name: "demo".into(),
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: Vec::new(),
            env: Vec::new(),
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        };
        let bytes = serde_json::to_vec(&meta).expect("serialize");
        std::fs::write(rec.dir().join(names::META), bytes).expect("write");
        let err = rec.metadata().expect_err("must fail");
        match err {
            ProcessError::MetadataIdMismatch { expected, found } => {
                assert_eq!(expected, rec.id());
                assert_eq!(found, other_id);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn list_under_skips_dot_dirs_and_invalid_names() {
        let tmp = TempDir::new().expect("tmp");
        let id1 = ProcessId::generate();
        let id2 = ProcessId::generate();
        std::fs::create_dir_all(tmp.path().join(id1.to_string())).expect("mkdir id1");
        std::fs::create_dir_all(tmp.path().join(id2.to_string())).expect("mkdir id2");
        std::fs::create_dir_all(tmp.path().join(".locks")).expect("mkdir locks");
        std::fs::create_dir_all(tmp.path().join("not-an-id")).expect("mkdir bad");
        let recs = list_under(tmp.path()).expect("list");
        let mut found_ids: Vec<String> = recs.iter().map(|r| r.id().to_string()).collect();
        found_ids.sort();
        let mut expected = vec![id1.to_string(), id2.to_string()];
        expected.sort();
        assert_eq!(found_ids, expected);
    }

    #[test]
    fn list_under_returns_empty_for_missing_root() {
        let tmp = TempDir::new().expect("tmp");
        let missing = tmp.path().join("does-not-exist");
        let recs = list_under(&missing).expect("list");
        assert!(recs.is_empty());
    }
}
