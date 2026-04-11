//! Backup/restore lifecycle plumbing for the per-agent project-local hook
//! modules.
//!
//! All four hook-based agents — [`ClaudeAgent`](crate::agent::ClaudeAgent),
//! [`CodexAgent`](crate::agent::CodexAgent), [`GeminiAgent`](crate::agent::GeminiAgent),
//! and [`CopilotAgent`](crate::agent::CopilotAgent) — install a Stop-style hook
//! under a project-local directory (`${cwd}/.claude/`, `${cwd}/.codex/`,
//! `${cwd}/.gemini/`, `${cwd}/.github/hooks/` respectively), let the
//! interactive CLI run, then finalize: read whatever the hook captured,
//! restore any user-authored files the installer overwrote, and remove
//! every scratch file produced by the install path.
//!
//! The actual JSON schema each CLI expects (and each transcript format we
//! parse afterward) is different enough that the four hook modules are
//! kept as siblings rather than a single generic. This module contains
//! only the shared low-level lifecycle pieces:
//!
//! * [`HookCapture`] — the tuple returned by every hook module's
//!   `finalize` path.
//! * [`BackupSlot`] — handles the "back up whatever was there, remember
//!   whether there was anything, restore on finalize" state machine for a
//!   single file. Each hook module owns one of these per file it
//!   overwrites.
//! * [`map_hook_io`] — funnel [`std::io::Error`] into
//!   [`AgentError::HookSetup`] with a short static label.
//! * [`make_executable`] — `chmod +x` for freshly written hook scripts.
//! * [`remove_if_exists`] — remove a file and ignore "not found".
//!
//! Every path handled here is expected to live **inside the workspace
//! directory** the agent was handed. None of these helpers ever touch
//! `~/.claude/`, `~/.codex/`, `~/.gemini/`, or `~/.github/`.

use std::path::{Path, PathBuf};

use tokio::fs;

use super::AgentError;

/// Environment variable name used by the three hook-bundle-driven agents
/// (Claude, Codex, Copilot) to communicate the absolute path of the state
/// file to the installed hook script. Gemini does not use this — its
/// `AfterAgent` hook receives `prompt_response` directly on stdin and
/// writes its state file at a path derived from `$PWD`.
pub(crate) const ITER_STATE_ENV: &str = "ITER_STATE_FILE";

/// What every hook module's `finalize` path returns. The `last_output`
/// field populates [`AgentReport::last_output`](crate::AgentReport)
/// and rides along on the `AgentFinished` event so debug UIs and event
/// sinks can peek at what the agent printed. `turn_count` populates
/// [`AgentReport::turn_count`](crate::AgentReport).
#[derive(Debug, Clone, Default)]
pub(crate) struct HookCapture {
    /// Last assistant text message extracted by the hook module, or
    /// `None` when the hook never fired (e.g. the CLI was killed before
    /// its Stop event, or the installed hook did not take effect).
    pub last_output: Option<String>,
    /// Number of assistant text messages the finalize path observed.
    /// `None` when the hook never fired. `Some(0)` means the hook fired
    /// but the transcript was empty or unparseable.
    pub turn_count: Option<u32>,
}

/// Backup-on-install + restore-on-finalize state machine for a single
/// file owned by a hook module.
///
/// Each hook module uses one of these per file it needs to overwrite.
/// The slot records three possible states in the on-disk bundle
/// directory:
///
/// * **Pre-existing** — the target file existed before `install`; its
///   bytes are copied to `<bundle_dir>/<backup_name>`.
/// * **Absent** — the target file did not exist before `install`; an
///   empty marker file is written to `<bundle_dir>/<absent_marker>` so
///   finalize knows to delete rather than restore.
/// * **Unused** — neither file is present; finalize is a no-op.
///
/// `install()` is called with the path of the target file; it handles
/// the actual backup. `restore()` is called during finalize; it reads
/// the backup (or absent marker) and restores the filesystem to its
/// pre-install state, cleaning up the slot on the way out.
#[derive(Debug)]
pub(crate) struct BackupSlot {
    /// Where the user's pre-existing file lives (the file we overwrite).
    target: PathBuf,
    /// `<bundle_dir>/<backup_name>` — where we stash the original bytes.
    backup: PathBuf,
    /// `<bundle_dir>/<backup_name>.absent` — marker that the target did
    /// not exist pre-install, so finalize should delete rather than
    /// restore.
    absent_marker: PathBuf,
}

impl BackupSlot {
    /// Construct a slot. `bundle_dir` must exist and be writable — the
    /// slot itself never creates parent directories. `target` is the
    /// path of the file being overwritten. `backup_name` is the short
    /// filename used inside `bundle_dir` to stash the original bytes;
    /// the absent-marker is derived by appending `".absent"`.
    pub(crate) fn new(bundle_dir: &Path, target: PathBuf, backup_name: &str) -> Self {
        let backup = bundle_dir.join(backup_name);
        let absent_marker = bundle_dir.join(format!("{backup_name}.absent"));
        Self {
            target,
            backup,
            absent_marker,
        }
    }

    /// Back up whatever is currently at the target path. Must be called
    /// before the hook module overwrites the file. Leaves the slot in
    /// either the "pre-existing" (backup present) or "absent" (marker
    /// present) state. Any stale files from a previous crashed run are
    /// cleared.
    pub(crate) async fn snapshot(&self) -> Result<(), AgentError> {
        match fs::read(&self.target).await {
            Ok(bytes) => {
                fs::write(&self.backup, &bytes)
                    .await
                    .map_err(map_hook_io("back up existing file"))?;
                drop(fs::remove_file(&self.absent_marker).await);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::write(&self.absent_marker, b"")
                    .await
                    .map_err(map_hook_io("write absent marker"))?;
                drop(fs::remove_file(&self.backup).await);
                Ok(())
            }
            Err(e) => Err(map_hook_io("read existing file")(e)),
        }
    }

    /// Restore the pre-install state at the target path. If a backup exists,
    /// its bytes are written back; if the absent marker exists, the
    /// target is deleted; if neither is present, the slot is a no-op
    /// (install was never called or already finalized). The slot's own
    /// scratch files are removed on success.
    pub(crate) async fn restore(&self) -> Result<(), AgentError> {
        match fs::read(&self.backup).await {
            Ok(bytes) => {
                fs::write(&self.target, bytes)
                    .await
                    .map_err(map_hook_io("restore file from backup"))?;
                fs::remove_file(&self.backup)
                    .await
                    .map_err(map_hook_io("remove backup file"))?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::metadata(&self.absent_marker).await {
                    Ok(_) => {
                        if let Err(e) = fs::remove_file(&self.target).await
                            && e.kind() != std::io::ErrorKind::NotFound
                        {
                            return Err(map_hook_io("remove synthesized file")(e));
                        }
                        fs::remove_file(&self.absent_marker)
                            .await
                            .map_err(map_hook_io("remove absent marker"))?;
                        Ok(())
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(map_hook_io("stat absent marker")(e)),
                }
            }
            Err(e) => Err(map_hook_io("read backup file")(e)),
        }
    }
}

/// Wrap a [`std::io::Error`] into [`AgentError::HookSetup`] with a short
/// static label describing the operation that failed.
pub(crate) fn map_hook_io(op: &'static str) -> impl FnOnce(std::io::Error) -> AgentError {
    move |e| AgentError::HookSetup(format!("{op}: {e}"))
}

/// `chmod +x` equivalent. Unix-only because the four hook protocols
/// iter supports are themselves unix-only in practice (they all execute
/// bash scripts the install path writes).
#[cfg(unix)]
pub(crate) async fn make_executable(path: &Path) -> Result<(), AgentError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .await
        .map_err(map_hook_io("stat hook script"))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
        .await
        .map_err(map_hook_io("chmod hook script"))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) async fn make_executable(_path: &Path) -> Result<(), AgentError> {
    Err(AgentError::HookSetup(
        "hook-driven interactive mode is only supported on unix-like systems".into(),
    ))
}

/// Remove a file, treating "not found" as success. Any other I/O error
/// is wrapped into [`AgentError::HookSetup`] with `op` as the label.
pub(crate) async fn remove_if_exists(path: &Path, op: &'static str) -> Result<(), AgentError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(map_hook_io(op)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn backup_slot_snapshot_and_restore_round_trip() {
        let tmp = TempDir::new().expect("tmp");
        let bundle_dir = tmp.path().join(".bundle");
        fs::create_dir_all(&bundle_dir).await.expect("mkdir");
        let target = tmp.path().join("target.json");
        fs::write(&target, b"original").await.expect("seed");

        let slot = BackupSlot::new(&bundle_dir, target.clone(), "target.json.bak");
        slot.snapshot().await.expect("snapshot");
        assert!(bundle_dir.join("target.json.bak").exists());

        fs::write(&target, b"overwritten")
            .await
            .expect("write replacement");
        slot.restore().await.expect("restore");

        let restored = fs::read(&target).await.expect("read");
        assert_eq!(restored, b"original");
        assert!(!bundle_dir.join("target.json.bak").exists());
    }

    #[tokio::test]
    async fn backup_slot_absent_marker_deletes_on_restore() {
        let tmp = TempDir::new().expect("tmp");
        let bundle_dir = tmp.path().join(".bundle");
        fs::create_dir_all(&bundle_dir).await.expect("mkdir");
        let target = tmp.path().join("synth.json");

        let slot = BackupSlot::new(&bundle_dir, target.clone(), "synth.json.bak");
        slot.snapshot().await.expect("snapshot absent");
        assert!(bundle_dir.join("synth.json.bak.absent").exists());

        // Hook module synthesizes the file.
        fs::write(&target, b"synth")
            .await
            .expect("write synthesized");
        slot.restore().await.expect("restore");

        assert!(!target.exists(), "synthesized file should be removed");
        assert!(!bundle_dir.join("synth.json.bak.absent").exists());
    }

    #[tokio::test]
    async fn backup_slot_restore_is_noop_without_snapshot() {
        let tmp = TempDir::new().expect("tmp");
        let bundle_dir = tmp.path().join(".bundle");
        fs::create_dir_all(&bundle_dir).await.expect("mkdir");
        let target = tmp.path().join("never.json");

        let slot = BackupSlot::new(&bundle_dir, target.clone(), "never.json.bak");
        // Never called snapshot, so restore should quietly do nothing.
        slot.restore().await.expect("noop");
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn remove_if_exists_is_noop_on_missing() {
        let tmp = TempDir::new().expect("tmp");
        remove_if_exists(&tmp.path().join("nope"), "op")
            .await
            .expect("noop");
    }
}
