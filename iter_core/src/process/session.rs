//! `ProcessSession` — the only constructor of `ProcessStatusFile`.
//!
//! Per rev17 §A1/§B2, intra-process sharing of the status fd happens via
//! `Arc<ProcessStatusFile>`. To make that invariant structural rather than
//! conventional, **only** [`ProcessSession::create_initial`] and
//! [`ProcessSession::adopt`] construct one. The runtime, the handle, and the
//! adoption path all receive their `Arc` from a session.
//!
//! Two factories cover the two writer sites:
//!
//! - [`create_initial`] — foreground startup *and* detached parent
//!   registration. Creates `~/.iter/proc/<id>/`, the initial `status` file
//!   (`initializing\n` under flock), and writes `meta.json`. The caller is
//!   responsible for any extra files (e.g. the parent of a detached spawn
//!   adds `bootstrap_token` before exec).
//!
//! - [`adopt`] — detached child opening the directory its parent created.
//!   Opens the existing `status` and re-reads `meta.json`. The actual
//!   `Initializing → Running` transition happens in
//!   [`crate::process::status_file::ProcessStatusFile::locked_adoption_write`],
//!   driven by `adoption::adopt_from_argv`.
//!
//! [`create_initial`]: ProcessSession::create_initial
//! [`adopt`]: ProcessSession::adopt

use std::path::Path;
use std::sync::Arc;

use crate::process::error::ProcessError;
use crate::process::id::ProcessId;
use crate::process::metadata::ProcessMetadata;
use crate::process::paths::{FILE_MODE, ProcPaths, names};
use crate::process::status_file::ProcessStatusFile;

/// Holder of the per-process directory triple: paths, status fd, metadata.
///
/// Cloning the `Arc<ProcessSession>` shares the same status fd; the runtime
/// passes one to the handle so they coordinate over a single Mutex-guarded
/// `File` (rev17 §B2).
#[derive(Debug)]
pub struct ProcessSession {
    paths: Arc<ProcPaths>,
    status_file: Arc<ProcessStatusFile>,
    metadata: ProcessMetadata,
}

impl ProcessSession {
    /// Create a fresh `~/.iter/proc/<id>/` and initialise its `status`
    /// (Initializing) + `meta.json`.
    ///
    /// Used by:
    /// - foreground `iter run` — the same OS process that will run the
    ///   runner. The runner later calls
    ///   `ProcessStatusFile::locked_initial_write` to flip
    ///   `Initializing → Running` after publishing the pid file.
    /// - the parent of `iter run --detach` — the spawner additionally
    ///   writes `bootstrap_token` before fork/exec; the child opens the same
    ///   directory via [`ProcessSession::adopt`].
    ///
    /// `metadata.id` must match the ULID under which the directory is
    /// allocated; this is checked structurally via `ProcPaths::create_for_new_id`
    /// using the same id.
    pub async fn create_initial(
        root: &Path,
        metadata: ProcessMetadata,
    ) -> Result<Arc<Self>, ProcessError> {
        let paths = ProcPaths::create_for_new_id(root, metadata.id)?;
        let status_file = ProcessStatusFile::create_initial_locked(paths.clone()).await?;
        write_metadata(&paths, &metadata)?;
        Ok(Arc::new(Self {
            paths,
            status_file,
            metadata,
        }))
    }

    /// Open the directory that a detached parent already created.
    ///
    /// This is the entry point for `adoption::adopt_from_argv`, which then
    /// validates the bootstrap token and runs
    /// `ProcessStatusFile::locked_adoption_write` to publish the pid file
    /// and flip `Initializing → Running`.
    pub async fn adopt(root: &Path, id: ProcessId) -> Result<Arc<Self>, ProcessError> {
        let paths = ProcPaths::open_existing(root, id)?;
        let status_file = ProcessStatusFile::open_for_existing(paths.clone()).await?;
        let metadata = read_metadata(&paths)?;
        if metadata.id != id {
            return Err(ProcessError::MetadataIdMismatch {
                expected: id,
                found: metadata.id,
            });
        }
        Ok(Arc::new(Self {
            paths,
            status_file,
            metadata,
        }))
    }

    /// Process directory paths (clonable `Arc`).
    pub fn paths(&self) -> Arc<ProcPaths> {
        self.paths.clone()
    }

    /// Shared status fd holder. Pass clones to handle/runtime so they
    /// serialise over the same `Mutex<File>` + flock (rev17 §B2/§B3).
    pub fn status_file(&self) -> Arc<ProcessStatusFile> {
        self.status_file.clone()
    }

    /// Frozen metadata for this session (kind, name, iterfile, args, env, …).
    pub fn metadata(&self) -> &ProcessMetadata {
        &self.metadata
    }

    /// Convenience accessor: this process's ULID.
    pub fn id(&self) -> ProcessId {
        self.metadata.id
    }

    /// Convenience accessor: the registered name.
    pub fn name(&self) -> &str {
        &self.metadata.name
    }
}

/// Write `meta.json` plus the plain-text side files (`name`, `iterfile`,
/// `subcommand`, `started_at`) exactly once each (`O_CREAT|O_EXCL|0600`).
///
/// All of these are immutable for the lifetime of the directory (rev17 §A1 —
/// updates happen in side-files like `status` and `pid`), so the simple
/// "create new" semantics are correct here. The plain-text companions exist
/// so consumers like `iter ps` can render rows without JSON parsing
/// (rev17 file-layout: `name`, `iterfile`, `subcommand`, `started_at`).
fn write_metadata(paths: &ProcPaths, metadata: &ProcessMetadata) -> Result<(), ProcessError> {
    let json = serde_json::to_vec_pretty(metadata).map_err(ProcessError::JsonWrite)?;
    // **Publication order matters.** `meta.json` is the file every external
    // consumer (compose discovery, `iter ps`, `iter inspect`) gates on,
    // because it is the only file that carries the structured labels and
    // ULID. Until very recently we wrote it FIRST, which opened a race
    // where a concurrent reader could see the labels but not yet see the
    // plain-text side files (`started_at`, `name`, …) and fail with
    // `ENOENT`. Writing `meta.json` LAST closes that window: any consumer
    // that successfully reads `meta.json` is guaranteed to also be able
    // to read every plain-text companion. The `write_excl` calls each
    // `fsync` so the inode order matches the call order.
    write_excl(
        paths,
        names::NAME,
        metadata.name.as_bytes(),
        /* trailing_newline */ true,
    )?;
    write_excl(
        paths,
        names::ITERFILE,
        metadata.iterfile.as_os_str().as_encoded_bytes(),
        /* trailing_newline */ true,
    )?;
    write_excl(
        paths,
        names::SUBCOMMAND,
        metadata.subcommand.as_bytes(),
        /* trailing_newline */ true,
    )?;
    write_excl(
        paths,
        names::STARTED_AT,
        metadata.started_at.to_rfc3339().as_bytes(),
        /* trailing_newline */ true,
    )?;
    write_excl(paths, names::META, &json, /* trailing_newline */ true)?;
    Ok(())
}

fn write_excl(
    paths: &ProcPaths,
    name: &str,
    bytes: &[u8],
    trailing_newline: bool,
) -> Result<(), ProcessError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = paths.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .mode(FILE_MODE)
        .open(&path)
        .map_err(ProcessError::Io)?;
    file.write_all(bytes).map_err(ProcessError::Io)?;
    if trailing_newline {
        file.write_all(b"\n").map_err(ProcessError::Io)?;
    }
    file.sync_all().map_err(ProcessError::Io)?;
    Ok(())
}

/// Read and parse `meta.json` from an existing process directory.
fn read_metadata(paths: &ProcPaths) -> Result<ProcessMetadata, ProcessError> {
    let path = paths.join(names::META);
    let bytes = std::fs::read(&path).map_err(ProcessError::Io)?;
    serde_json::from_slice(&bytes).map_err(ProcessError::JsonRead)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sample_metadata(id: ProcessId, name: &str) -> ProcessMetadata {
        ProcessMetadata {
            id,
            name: name.to_owned(),
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into()],
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn create_initial_writes_status_and_metadata() {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let session = ProcessSession::create_initial(tmp.path(), sample_metadata(id, "alpha"))
            .await
            .expect("create_initial");

        // status fd is open and ready; can read back via the public async API.
        let status = session.status_file().read_status().await.expect("read");
        assert_eq!(status, crate::process::status::ProcessStatus::Initializing);

        // metadata round-trips through the JSON file.
        let on_disk = read_metadata(&session.paths()).expect("read meta");
        assert_eq!(on_disk.id, id);
        assert_eq!(on_disk.name, "alpha");
        assert_eq!(session.id(), id);
        assert_eq!(session.name(), "alpha");
    }

    #[tokio::test]
    async fn create_initial_rejects_double_create() {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let _first = ProcessSession::create_initial(tmp.path(), sample_metadata(id, "alpha"))
            .await
            .expect("first");
        let err = ProcessSession::create_initial(tmp.path(), sample_metadata(id, "alpha"))
            .await
            .expect_err("second should fail");
        assert!(matches!(err, ProcessError::Io(_)));
    }

    #[tokio::test]
    async fn adopt_reopens_existing_session() {
        let tmp = TempDir::new().unwrap();
        let id = ProcessId::generate();
        let parent = ProcessSession::create_initial(tmp.path(), sample_metadata(id, "alpha"))
            .await
            .expect("create");

        // Drop the parent session and open it from scratch (simulates the
        // detached-child flow: parent exited after writing the directory).
        drop(parent);

        let child = ProcessSession::adopt(tmp.path(), id).await.expect("adopt");
        assert_eq!(child.id(), id);
        assert_eq!(child.name(), "alpha");
        assert_eq!(
            child.status_file().read_status().await.expect("read"),
            crate::process::status::ProcessStatus::Initializing
        );
    }

    #[tokio::test]
    async fn adopt_detects_id_mismatch() {
        let tmp = TempDir::new().unwrap();
        let id_a = ProcessId::generate();
        // Hand-write a directory whose meta.json claims a different id.
        let id_b = ProcessId::generate();
        let _real = ProcessSession::create_initial(tmp.path(), sample_metadata(id_a, "alpha"))
            .await
            .expect("create");
        // Tamper: rewrite meta.json so id != directory id.
        let path = tmp.path().join(id_a.to_string()).join(names::META);
        let tampered = sample_metadata(id_b, "alpha");
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();

        let err = ProcessSession::adopt(tmp.path(), id_a)
            .await
            .expect_err("mismatch should fail");
        match err {
            ProcessError::MetadataIdMismatch { expected, found } => {
                assert_eq!(expected, id_a);
                assert_eq!(found, id_b);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
