//! Project-local Stop hook plumbing for [`ClaudeAgent`](crate::agent::ClaudeAgent)'s
//! interactive mode.
//!
//! iter's interactive Claude mode is a direct descendant of
//! [`agent-loop/claude-loop`](https://github.com/k-kinzal/agent-loop)'s
//! wrapper. The key invariant — copied verbatim from that script — is that
//! **everything lives under `${cwd}/.claude/`**, never under `~/.claude/`.
//! Writing to the global config would silently affect every other Claude Code
//! session on the machine; instead we mutate the project-local settings,
//! back up whatever was already there (via [`BackupSlot`]), and restore it
//! when the run ends.
//!
//! # Layout
//!
//! ```text
//! ${cwd}/.claude/
//!   settings.json                 # overwritten, previous content backed up
//!   hooks/iter-stop-hook.sh       # the Stop hook body (executable)
//!   iter-state.json               # Stop event payload captured from stdin
//!   .iter-bundle/
//!     settings.json.bak           # backup of any pre-existing settings.json
//! ```
//!
//! # Flow
//!
//! 1. [`HookBundle::install`] is called once before spawning `claude`. It
//!    creates the directory tree, backs up any pre-existing `settings.json`,
//!    writes a fresh settings file pointing at the hook script, and writes
//!    the hook script body itself. An `ITER_STATE_FILE` environment
//!    variable is returned alongside the handle so the spawned `claude`
//!    process can forward it to the hook.
//! 2. `claude` runs. When its first Stop event fires it invokes the hook,
//!    which writes the event's stdin JSON to `iter-state.json` and prints
//!    `{"continue": false, ...}` so `claude` exits cleanly.
//! 3. [`HookBundle::finalize`] reads `iter-state.json`, walks the referenced
//!    transcript JSONL via
//!    [`parse_claude_transcript`](crate::agent::transcript::parse_claude_transcript),
//!    restores the settings backup (if any) or deletes the synthesized one,
//!    and removes every scratch file it created. Returns a [`HookCapture`]
//!    describing what the hook saw.
//!
//! The module is intentionally agnostic about how `claude` is spawned; the
//! caller wires the spawn itself. Tests in this module only exercise the
//! filesystem state machine, leaving the subprocess plumbing to
//! `claude.rs`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, HookCapture, ITER_STATE_ENV, make_executable, map_hook_io, remove_if_exists,
};
use crate::agent::transcript::parse_claude_transcript;

/// Relative layout under `${cwd}/.claude/`. These constants are centralized
/// so test assertions can match the same paths the production code writes.
const CLAUDE_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const HOOK_SCRIPT_REL: &str = "hooks/iter-stop-hook.sh";
const STATE_FILE_REL: &str = "iter-state.json";
const BUNDLE_DIR: &str = ".iter-bundle";
const SETTINGS_BACKUP_NAME: &str = "settings.json.bak";

/// Bash source of the Stop hook. Deliberately minimal — it copies stdin
/// verbatim into the state file and tells `claude` to exit. All transcript
/// parsing happens in Rust after the child exits, which keeps this file
/// free of `jq`/`python` runtime dependencies.
const HOOK_SCRIPT_BODY: &str = r#"#!/usr/bin/env bash
# iter Stop hook for Claude Code — installed by iter_core::agent::ClaudeAgent.
# Writes the Stop event payload (stdin JSON) into $ITER_STATE_FILE and
# tells Claude Code to exit by printing {"continue": false, ...}.
#
# CLAUDE_PROJECT_DIR is set by Claude Code to the repo root containing
# the .claude/ directory. We use it as a fallback so the hook is robust to
# users who run claude from a subdirectory.
set -euo pipefail

STATE_FILE="${ITER_STATE_FILE:-${CLAUDE_PROJECT_DIR:-$PWD}/.claude/iter-state.json}"
mkdir -p "$(dirname "$STATE_FILE")"
# Slurp stdin verbatim. Rust post-processes it after claude exits.
cat > "$STATE_FILE"
printf '%s\n' '{"continue": false, "stopReason": "iter stop hook captured"}'
"#;

/// Handle returned from [`HookBundle::install`]. Dropping it does **not**
/// clean up the filesystem — callers must invoke [`HookBundle::finalize`]
/// explicitly so that restore errors can surface to the runner.
#[derive(Debug)]
pub(crate) struct HookBundle {
    /// Absolute path to `${cwd}/.claude/`.
    claude_dir: PathBuf,
    /// Absolute path to the state file the hook writes into.
    state_file: PathBuf,
    /// Backup slot for the user's pre-existing `settings.json`, if any.
    settings_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local Stop hook bundle under `cwd`.
    ///
    /// Creates `${cwd}/.claude/` and its required subdirectories, backs up
    /// any pre-existing `settings.json` into `${cwd}/.claude/.iter-bundle/`,
    /// writes a fresh `settings.json` whose Stop hook entry points at the
    /// hook script, and writes the hook script body itself with mode
    /// `0o755`. Returns a handle plus the path that should be exported as
    /// the `ITER_STATE_FILE` environment variable on the spawned `claude`
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::HookSetup`] on any filesystem / JSON failure
    /// along the install path. No partial state is left behind: the caller
    /// can retry or fall back to print mode.
    pub(crate) async fn install(cwd: &Path) -> Result<Self, AgentError> {
        let claude_dir = cwd.join(CLAUDE_DIR);
        let bundle_dir = claude_dir.join(BUNDLE_DIR);
        let settings_path = claude_dir.join(SETTINGS_FILE);
        let hook_script = claude_dir.join(HOOK_SCRIPT_REL);
        let state_file = claude_dir.join(STATE_FILE_REL);

        // Create the directory tree — `.claude/`, `.claude/hooks/`,
        // `.claude/.iter-bundle/`. `create_dir_all` is a no-op when any
        // segment already exists, so this is safe on repeat installs.
        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create .iter-bundle directory"))?;
        if let Some(parent) = hook_script.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(map_hook_io("create hooks directory"))?;
        }

        // Back up any pre-existing settings.json via the shared slot.
        let settings_slot =
            BackupSlot::new(&bundle_dir, settings_path.clone(), SETTINGS_BACKUP_NAME);
        settings_slot.snapshot().await?;

        // Write the fresh settings.json. We do NOT attempt to merge with
        // whatever the user had — the backup is the source of truth, we
        // own the file for the duration of the run. The shape of this
        // payload matches Claude Code's documented Stop hook schema.
        let hook_cmd = hook_script
            .to_str()
            .ok_or_else(|| {
                AgentError::HookSetup(format!(
                    "hook script path is not valid UTF-8: {}",
                    hook_script.display()
                ))
            })?
            .to_owned();
        let settings_payload = json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": hook_cmd }
                        ]
                    }
                ]
            }
        });
        let settings_bytes = serde_json::to_vec_pretty(&settings_payload)
            .map_err(|e| AgentError::HookSetup(format!("serialize settings.json: {e}")))?;
        fs::write(&settings_path, settings_bytes)
            .await
            .map_err(map_hook_io("write synthesized settings.json"))?;

        // Write the hook script body + chmod +x. We always overwrite so
        // repeat installs pick up any body changes between iter releases.
        fs::write(&hook_script, HOOK_SCRIPT_BODY)
            .await
            .map_err(map_hook_io("write hook script"))?;
        make_executable(&hook_script).await?;

        // Ensure no stale state file leaks into this run. Finalize reads
        // the file's presence as "the hook fired at least once".
        if let Err(e) = fs::remove_file(&state_file).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(map_hook_io("clear stale iter-state.json")(e));
        }

        Ok(Self {
            claude_dir,
            state_file,
            settings_slot,
        })
    }

    /// Path of the state file the hook writes into. Callers export this as
    /// the `ITER_STATE_FILE` environment variable on the spawned `claude`
    /// process so the hook body can locate it. Production callers go through
    /// [`Self::env_var`]; this accessor exists solely for test assertions
    /// that need to poke at the state file directly.
    #[cfg(test)]
    pub(crate) fn state_file(&self) -> &Path {
        &self.state_file
    }

    /// Environment variable name / value pair the caller should set on the
    /// spawned `claude` process.
    pub(crate) fn env_var(&self) -> (&'static str, &Path) {
        (ITER_STATE_ENV, &self.state_file)
    }

    /// Read whatever the hook captured, restore the original settings
    /// file, and remove every scratch file the install path wrote.
    ///
    /// This is a single-shot consume operation — the handle is moved by
    /// value so callers cannot accidentally double-finalize.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::HookSetup`] on any filesystem failure during
    /// cleanup, or [`AgentError::HookStateParse`] when the state file was
    /// written but its contents could not be decoded.
    pub(crate) async fn finalize(self) -> Result<HookCapture, AgentError> {
        let capture = self.read_capture().await?;
        self.settings_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(capture)
    }

    /// Parse whatever the hook captured.
    async fn read_capture(&self) -> Result<HookCapture, AgentError> {
        let state_bytes = match fs::read(&self.state_file).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Hook never fired — e.g. claude was killed before its
                // Stop event, or the installed settings never took effect.
                return Ok(HookCapture::default());
            }
            Err(e) => return Err(map_hook_io("read iter-state.json")(e)),
        };

        let payload: Value = serde_json::from_slice(&state_bytes)
            .map_err(|e| AgentError::HookStateParse(format!("decode iter-state.json: {e}")))?;

        let transcript_path = payload
            .get("transcript_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);

        let Some(transcript) = transcript_path else {
            return Ok(HookCapture {
                last_output: None,
                turn_count: Some(0),
            });
        };

        parse_claude_transcript(&transcript).await
    }

    /// Delete the hook script, state file, and the `.iter-bundle/`
    /// directory so repeated runs start from a clean slate.
    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.claude_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove hook script").await?;

        // Prune the hooks/ directory if it is now empty — preserves any
        // user-authored hooks that happen to live alongside ours.
        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await); // best-effort
        }

        remove_if_exists(&self.state_file, "remove iter-state.json").await?;

        let bundle_dir = self.claude_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove .iter-bundle directory")(e)),
        }

        // Finally prune the `.claude/` directory itself if it ended up
        // empty. Best-effort: users who keep their own files there will
        // see it persist.
        drop(fs::remove_dir(&self.claude_dir).await);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, bytes).await.expect("write");
    }

    #[tokio::test]
    async fn install_creates_settings_hook_and_state_dir() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        let settings = tmp.path().join(".claude/settings.json");
        assert!(settings.exists(), "settings.json must exist");
        let parsed: Value =
            serde_json::from_slice(&fs::read(&settings).await.expect("read")).expect("json");
        let hook_cmd = parsed["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            hook_cmd.ends_with("iter-stop-hook.sh"),
            "hook command must point at iter-stop-hook.sh, got {hook_cmd}"
        );

        let script = tmp.path().join(".claude/hooks/iter-stop-hook.sh");
        assert!(script.exists(), "hook script must exist");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&script)
                .await
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "hook script must be executable");
        }

        assert_eq!(
            bundle.state_file(),
            tmp.path().join(".claude/iter-state.json")
        );
        let (env_key, env_val) = bundle.env_var();
        assert_eq!(env_key, "ITER_STATE_FILE");
        assert_eq!(env_val, bundle.state_file());
    }

    #[tokio::test]
    async fn install_backs_up_existing_settings_and_finalize_restores_it() {
        let tmp = TempDir::new().expect("tmp");
        let settings_path = tmp.path().join(".claude/settings.json");
        let original = json!({ "theme": "dark", "unrelated": true });
        write(
            &settings_path,
            serde_json::to_vec_pretty(&original).unwrap().as_slice(),
        )
        .await;

        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        // Our settings.json is now the synthesized one — different from
        // the original.
        let current: Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_ne!(current, original, "settings.json must have been replaced");
        assert!(
            tmp.path()
                .join(".claude/.iter-bundle/settings.json.bak")
                .exists(),
            "backup file must exist"
        );

        bundle.finalize().await.expect("finalize");

        let restored: Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, original,
            "finalize must restore the original settings.json",
        );
        assert!(
            !tmp.path().join(".claude/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up on finalize"
        );
        assert!(
            !tmp.path().join(".claude/hooks").exists(),
            "empty hooks dir must be cleaned up on finalize"
        );
    }

    #[tokio::test]
    async fn finalize_deletes_synthesized_settings_when_none_existed() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        assert!(tmp.path().join(".claude/settings.json").exists());
        bundle.finalize().await.expect("finalize");
        assert!(
            !tmp.path().join(".claude/settings.json").exists(),
            "synthesized settings.json must be removed when no backup existed"
        );
    }

    #[tokio::test]
    async fn finalize_without_state_returns_empty_capture() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        let capture = bundle.finalize().await.expect("finalize");
        assert!(capture.last_output.is_none());
        assert!(capture.turn_count.is_none());
    }

    #[tokio::test]
    async fn finalize_parses_transcript_last_assistant_message() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        // Fake a transcript JSONL next to .claude/.
        let transcript = tmp.path().join("transcript.jsonl");
        let lines = [
            json!({
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "text", "text": "first turn"}
                    ]
                }
            }),
            json!({"type": "user", "message": {"content": []}}),
            json!({
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "text", "text": "final answer"}
                    ]
                }
            }),
        ];
        let mut body = String::new();
        for line in &lines {
            body.push_str(&serde_json::to_string(line).unwrap());
            body.push('\n');
        }
        fs::write(&transcript, body)
            .await
            .expect("write transcript");

        // Fake the hook firing: write its stdin payload into the state
        // file just like the shell hook would.
        let state_payload = json!({
            "session_id": "abc",
            "transcript_path": transcript.to_str().unwrap(),
            "stop_hook_active": false,
        });
        fs::write(
            bundle.state_file(),
            serde_json::to_vec(&state_payload).unwrap(),
        )
        .await
        .expect("write state");

        let capture = bundle.finalize().await.expect("finalize");
        assert_eq!(capture.last_output.as_deref(), Some("final answer"));
        assert_eq!(capture.turn_count, Some(2));
    }

    #[tokio::test]
    async fn finalize_rejects_malformed_state_file() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        fs::write(bundle.state_file(), b"not json at all")
            .await
            .expect("write state");
        let err = bundle.finalize().await.expect_err("must reject");
        assert!(
            matches!(err, AgentError::HookStateParse(_)),
            "expected HookStateParse, got {err:?}"
        );
    }
}
