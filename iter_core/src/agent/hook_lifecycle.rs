//! Backup/restore lifecycle plumbing and project-scoped state directory
//! helpers for the per-agent hook modules.
//!
//! All four hook-based agents — [`ClaudeAgent`](crate::agent::ClaudeAgent),
//! [`CodexAgent`](crate::agent::CodexAgent), [`GeminiAgent`](crate::agent::GeminiAgent),
//! and [`CopilotAgent`](crate::agent::CopilotAgent) — install a Stop-style hook
//! under a project-local directory (`${cwd}/.claude/`, `${cwd}/.codex/`,
//! `${cwd}/.gemini/`, `${cwd}/.github/hooks/` respectively), let the
//! interactive CLI run, then finalize: restore any user-authored files the
//! installer overwrote and remove every scratch file produced by the install
//! path.
//!
//! Hook sidecar files (backed-up user hooks, installed scripts that need to
//! survive across the install → run → finalize boundary) live under
//! `~/.iter/projects/<project-id>/<service>/hooks/`, never inside the
//! workspace. See [`project_hooks_dir`] for the directory layout.
//!
//! This module contains only the shared low-level lifecycle pieces:
//!
//! * [`BackupSlot`] — handles the "back up whatever was there, remember
//!   whether there was anything, restore on finalize" state machine for a
//!   single file.
//! * [`project_hooks_dir`] — resolves the per-project, per-service hooks
//!   sidecar directory under `~/.iter/projects/`.
//! * [`map_hook_io`] — funnel [`std::io::Error`] into
//!   [`AgentError::Launch`] with a short static label.
//! * [`make_executable`] — `chmod +x` for freshly written hook scripts.
//! * [`remove_if_exists`] — remove a file and ignore "not found".

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;

use super::AgentError;

/// Compute a project-scoped hooks sidecar directory.
///
/// The returned path is:
///
/// ```text
/// ~/.iter/projects/<project-id>/<service>/hooks/
/// ```
///
/// where `<project-id>` is `<basename>-<sha256(canonical_path)[:8]>`.
/// The basename prefix keeps `ls ~/.iter/projects/` human-readable;
/// the hash suffix prevents collisions across different paths sharing
/// the same directory name.
///
/// `workspace_path` is the agent's working directory (the value of
/// `AgentRunContext::workspace_path`). `service` is the compose service
/// name in compose mode, or `"default"` for standalone `iter run`.
///
/// # Errors
///
/// Returns [`AgentError::Launch`] if the home directory cannot be
/// resolved or the workspace path cannot be canonicalized.
pub(crate) fn project_hooks_dir(
    workspace_path: &Path,
    service: &str,
) -> Result<PathBuf, AgentError> {
    let home = home_dir().ok_or_else(|| {
        AgentError::Launch("could not resolve home directory".into())
    })?;
    let canonical = workspace_path.canonicalize().map_err(|e| {
        AgentError::Launch(format!(
            "canonicalize workspace path {}: {e}",
            workspace_path.display()
        ))
    })?;
    let project_id = compute_project_id(&canonical);
    Ok(home
        .join(".iter")
        .join("projects")
        .join(project_id)
        .join(service)
        .join("hooks"))
}

/// `<basename>-<sha256(canonical_path)[:8]>`.
fn compute_project_id(canonical_path: &Path) -> String {
    let basename = canonical_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let mut hasher = Sha256::new();
    hasher.update(canonical_path.as_os_str().as_encoded_bytes());
    let hash = hasher.finalize();
    let hash_prefix = hex::encode(&hash[..4]);
    format!("{basename}-{hash_prefix}")
}

fn home_dir() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    None
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

/// Wrap a [`std::io::Error`] into [`AgentError::Launch`] with a short
/// static label describing the operation that failed.
pub(crate) fn map_hook_io(op: &'static str) -> impl FnOnce(std::io::Error) -> AgentError {
    move |e| AgentError::Launch(format!("{op}: {e}"))
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
    Err(AgentError::Launch(
        "hook-driven interactive mode is only supported on unix-like systems".into(),
    ))
}

/// Escape a string for safe embedding in a bash single-quoted context.
///
/// The only character that cannot appear inside `'…'` is the single
/// quote itself. We handle it with the classic `'\''` (end quote,
/// escaped literal quote, re-open quote) idiom.
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Remove a file, treating "not found" as success. Any other I/O error
/// is wrapped into [`AgentError::Launch`] with `op` as the label.
pub(crate) async fn remove_if_exists(path: &Path, op: &'static str) -> Result<(), AgentError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(map_hook_io(op)(e)),
    }
}

/// Read any user-registered hook commands from an agent's config file
/// and write them to a sidecar script that the installed hook runs
/// first.
///
/// Returns the path of the written sidecar (for embedding in the
/// installed hook script), or `None` if the user had no pre-existing
/// hooks to preserve.
///
/// `config_path` is the agent's config file (e.g. `.claude/settings.json`).
/// `hook_event` is the JSON key for the hook event (e.g. `"Stop"`,
/// `"AfterAgent"`, `"agentStop"`).
/// `hooks_dir` is the project-scoped hooks sidecar directory returned
/// by [`project_hooks_dir`].
pub(crate) async fn extract_user_hooks(
    config_path: &Path,
    hook_event: &str,
    hooks_dir: &Path,
) -> Result<Option<PathBuf>, AgentError> {
    let config_bytes = match fs::read(config_path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(map_hook_io("read agent config for user hooks")(e)),
    };

    let config: serde_json::Value = serde_json::from_slice(&config_bytes)
        .map_err(|e| AgentError::Launch(format!("parse agent config: {e}")))?;

    let commands = collect_hook_commands(&config, hook_event);
    if commands.is_empty() {
        return Ok(None);
    }

    fs::create_dir_all(hooks_dir)
        .await
        .map_err(map_hook_io("create project hooks sidecar directory"))?;

    let sidecar = hooks_dir.join("existing-stop-hooks.sh");
    let mut script = String::from("#!/usr/bin/env bash\nset -euo pipefail\n");
    for cmd in &commands {
        script.push_str(cmd);
        script.push('\n');
    }
    fs::write(&sidecar, script.as_bytes())
        .await
        .map_err(map_hook_io("write existing-stop-hooks.sh sidecar"))?;
    make_executable(&sidecar).await?;

    Ok(Some(sidecar))
}

/// Walk the config JSON and extract command strings from the hook event.
///
/// Supports two shapes:
/// - Claude/Gemini: `hooks.<event>[].hooks[].command`
/// - Codex: `hooks.<event>[].hooks[].command`
/// - Copilot: `hooks.<event>[].bash`
fn collect_hook_commands(config: &serde_json::Value, hook_event: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let Some(hooks_obj) = config.get("hooks") else {
        return commands;
    };
    let Some(event_array) = hooks_obj.get(hook_event).and_then(|v| v.as_array()) else {
        return commands;
    };
    for group in event_array {
        // Claude/Codex/Gemini shape: group.hooks[].command
        if let Some(inner_hooks) = group.get("hooks").and_then(|v| v.as_array()) {
            for hook in inner_hooks {
                if let Some(cmd) = hook.get("command").and_then(|v| v.as_str()) {
                    commands.push(cmd.to_owned());
                }
            }
        }
        // Copilot shape: group.bash
        if let Some(bash) = group.get("bash").and_then(|v| v.as_str()) {
            commands.push(bash.to_owned());
        }
    }
    commands
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

    #[test]
    fn project_id_is_deterministic() {
        let id1 = compute_project_id(Path::new("/Users/ab/Projects/iter"));
        let id2 = compute_project_id(Path::new("/Users/ab/Projects/iter"));
        assert_eq!(id1, id2);
        assert!(id1.starts_with("iter-"), "expected iter-<hash>, got {id1}");
        assert_eq!(id1.len(), "iter-".len() + 8);
    }

    #[test]
    fn project_id_differs_for_different_paths() {
        let id1 = compute_project_id(Path::new("/Users/ab/Projects/iter"));
        let id2 = compute_project_id(Path::new("/Users/ab/Projects/other"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn project_id_handles_same_basename_different_parent() {
        let id1 = compute_project_id(Path::new("/a/foo"));
        let id2 = compute_project_id(Path::new("/b/foo"));
        assert_ne!(id1, id2, "same basename but different paths should differ");
        assert!(id1.starts_with("foo-"));
        assert!(id2.starts_with("foo-"));
    }

    #[tokio::test]
    async fn extract_user_hooks_returns_none_for_missing_config() {
        let tmp = TempDir::new().expect("tmp");
        let hooks_dir = tmp.path().join("hooks");
        let result = extract_user_hooks(
            &tmp.path().join("nonexistent.json"),
            "Stop",
            &hooks_dir,
        )
        .await
        .expect("extract");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn extract_user_hooks_returns_none_for_config_without_hooks() {
        let tmp = TempDir::new().expect("tmp");
        let config = tmp.path().join("settings.json");
        fs::write(&config, b"{}").await.expect("write");
        let hooks_dir = tmp.path().join("hooks");
        let result = extract_user_hooks(&config, "Stop", &hooks_dir)
            .await
            .expect("extract");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn extract_user_hooks_writes_sidecar_for_claude_shape() {
        let tmp = TempDir::new().expect("tmp");
        let config = tmp.path().join("settings.json");
        let config_json = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo user hook 1" },
                            { "type": "command", "command": "echo user hook 2" }
                        ]
                    }
                ]
            }
        });
        fs::write(&config, serde_json::to_vec_pretty(&config_json).unwrap())
            .await
            .expect("write");
        let hooks_dir = tmp.path().join("hooks");
        let result = extract_user_hooks(&config, "Stop", &hooks_dir)
            .await
            .expect("extract");
        assert!(result.is_some());
        let sidecar = result.unwrap();
        let body = fs::read_to_string(&sidecar).await.expect("read sidecar");
        assert!(body.contains("echo user hook 1"));
        assert!(body.contains("echo user hook 2"));
    }

    #[tokio::test]
    async fn extract_user_hooks_writes_sidecar_for_copilot_shape() {
        let tmp = TempDir::new().expect("tmp");
        let config = tmp.path().join("copilot-loop.json");
        let config_json = serde_json::json!({
            "version": 1,
            "hooks": {
                "agentStop": [
                    { "type": "command", "bash": "./my-hook.sh" }
                ]
            }
        });
        fs::write(&config, serde_json::to_vec_pretty(&config_json).unwrap())
            .await
            .expect("write");
        let hooks_dir = tmp.path().join("hooks");
        let result = extract_user_hooks(&config, "agentStop", &hooks_dir)
            .await
            .expect("extract");
        assert!(result.is_some());
        let sidecar = result.unwrap();
        let body = fs::read_to_string(&sidecar).await.expect("read sidecar");
        assert!(body.contains("./my-hook.sh"));
    }
}
