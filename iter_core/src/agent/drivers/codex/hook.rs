//! Project-local Stop hook plumbing for [`CodexAgent`](crate::agent::CodexAgent)'s
//! interactive mode.
//!
//! iter's interactive Codex mode is a descendant of
//! [`agent-loop/codex-loop`](https://github.com/k-kinzal/agent-loop). Codex
//! ships a Claude-Code-style Stop hook behind a feature flag
//! (`-c "features.codex_hooks=true"` on the CLI); its transcript schema is
//! identical to Claude's (assistant events with `message.content[].text`)
//! so we reuse
//! [`parse_claude_transcript`](crate::agent::transcript::parse_claude_transcript).
//!
//! The key invariant — identical to the Claude module's — is that
//! **everything lives under `${cwd}/.codex/`**, never under `~/.codex/`.
//! Writing to the global config would silently affect every other Codex
//! session on the machine; instead we mutate the project-local settings,
//! back up whatever was already there (via
//! [`BackupSlot`](crate::agent::hook_lifecycle::BackupSlot)), and restore it when the
//! run ends.
//!
//! # Layout
//!
//! ```text
//! ${cwd}/.codex/
//!   hooks.json                    # overwritten, previous content backed up
//!   hooks/codex-loop-hook.sh      # the Stop hook body (executable)
//!   iter-state.json               # Stop event payload captured from stdin
//!   .iter-bundle/
//!     hooks.json.bak              # backup of any pre-existing hooks.json
//! ```
//!
//! # Flow
//!
//! 1. [`HookBundle::install`] writes the bundle layout and returns a
//!    handle plus the `ITER_STATE_FILE` environment variable pair. The
//!    caller spawns `codex` with `-c "features.codex_hooks=true"` and the
//!    env var set; that constant lives on the `CodexAgent` side, not here.
//! 2. When the first Stop event fires, the hook script writes the event's
//!    stdin JSON to `iter-state.json`, prints
//!    `{"continue": false, ...}` to stdout so Codex exits cleanly, and
//!    issues `kill -TERM $PPID` as a belt-and-suspenders against Codex
//!    builds that ignore the JSON response. `$PPID` inside the hook is
//!    the Codex process iter spawned; iter itself is unaffected.
//! 3. [`HookBundle::finalize`] reads `iter-state.json`, walks the
//!    referenced transcript JSONL, restores the `hooks.json` backup (if
//!    any) or deletes the synthesized one, and removes every scratch
//!    file it created. Returns a
//!    [`HookCapture`](crate::agent::hook_lifecycle::HookCapture) describing what the
//!    hook saw.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, HookCapture, ITER_STATE_ENV, make_executable, map_hook_io, remove_if_exists,
};
use crate::agent::transcript::parse_claude_transcript;

/// Relative layout under `${cwd}/.codex/`. Centralized so test assertions
/// match the same paths the production code writes.
const CODEX_DIR: &str = ".codex";
const HOOKS_FILE: &str = "hooks.json";
const HOOK_SCRIPT_REL: &str = "hooks/codex-loop-hook.sh";
const STATE_FILE_REL: &str = "iter-state.json";
const BUNDLE_DIR: &str = ".iter-bundle";
const HOOKS_BACKUP_NAME: &str = "hooks.json.bak";

/// Bash source of the Codex Stop hook. Like the Claude variant this is
/// deliberately minimal: slurp stdin into the state file, print a JSON
/// continuation response to stdout so Codex exits, then issue `SIGTERM`
/// to the parent process (= the Codex child iter spawned) as a fallback
/// for Codex builds that ignore the continuation response.
const HOOK_SCRIPT_BODY: &str = r#"#!/usr/bin/env bash
# iter Stop hook for Codex — installed by iter_core::agent::CodexAgent.
# Writes the Stop event payload (stdin JSON) into $ITER_STATE_FILE and
# tells Codex to exit, falling back to SIGTERM on the Codex parent PID if
# the continuation response is ignored.
set -euo pipefail

STATE_FILE="${ITER_STATE_FILE:-$PWD/.codex/iter-state.json}"
mkdir -p "$(dirname "$STATE_FILE")"
cat > "$STATE_FILE"
printf '%s\n' '{"continue": false, "stopReason": "iter stop hook captured"}'

# Best-effort: SIGTERM our parent (= the Codex CLI iter spawned) so the
# TUI exits even when the JSON response is ignored. Suppress errors so a
# race-lost kill does not turn a clean Stop event into a hook failure.
if [[ -n "${PPID:-}" ]]; then
    kill -TERM "$PPID" 2>/dev/null || true
fi
"#;

/// Handle returned from [`HookBundle::install`]. Dropping it does **not**
/// clean up the filesystem — callers must invoke [`HookBundle::finalize`]
/// explicitly so restore errors can surface to the runner.
#[derive(Debug)]
pub(crate) struct HookBundle {
    /// Absolute path to `${cwd}/.codex/`.
    codex_dir: PathBuf,
    /// Absolute path to the state file the hook writes into.
    state_file: PathBuf,
    /// Backup slot for the user's pre-existing `hooks.json`, if any.
    hooks_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local Stop hook bundle under `cwd`.
    ///
    /// Creates `${cwd}/.codex/` and its required subdirectories, backs up
    /// any pre-existing `hooks.json` into `${cwd}/.codex/.iter-bundle/`,
    /// writes a fresh `hooks.json` whose Stop hook entry points at the
    /// hook script, and writes the hook script body itself with mode
    /// `0o755`.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::HookSetup`] on any filesystem / JSON failure
    /// along the install path. No partial state is left behind.
    pub(crate) async fn install(cwd: &Path) -> Result<Self, AgentError> {
        let codex_dir = cwd.join(CODEX_DIR);
        let bundle_dir = codex_dir.join(BUNDLE_DIR);
        let hooks_path = codex_dir.join(HOOKS_FILE);
        let hook_script = codex_dir.join(HOOK_SCRIPT_REL);
        let state_file = codex_dir.join(STATE_FILE_REL);

        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create .iter-bundle directory"))?;
        if let Some(parent) = hook_script.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(map_hook_io("create hooks directory"))?;
        }

        let hooks_slot = BackupSlot::new(&bundle_dir, hooks_path.clone(), HOOKS_BACKUP_NAME);
        hooks_slot.snapshot().await?;

        // Build the Codex hook configuration pointing at our script.
        let hook_cmd = hook_script
            .to_str()
            .ok_or_else(|| {
                AgentError::HookSetup(format!(
                    "hook script path is not valid UTF-8: {}",
                    hook_script.display()
                ))
            })?
            .to_owned();
        let hooks_payload = json!({
            "hooks": {
                "Stop": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": hook_cmd,
                                "timeout": 30
                            }
                        ]
                    }
                ]
            }
        });
        let hooks_bytes = serde_json::to_vec_pretty(&hooks_payload)
            .map_err(|e| AgentError::HookSetup(format!("serialize codex hooks.json: {e}")))?;
        fs::write(&hooks_path, hooks_bytes)
            .await
            .map_err(map_hook_io("write synthesized codex hooks.json"))?;

        fs::write(&hook_script, HOOK_SCRIPT_BODY)
            .await
            .map_err(map_hook_io("write codex hook script"))?;
        make_executable(&hook_script).await?;

        if let Err(e) = fs::remove_file(&state_file).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(map_hook_io("clear stale codex iter-state.json")(e));
        }

        Ok(Self {
            codex_dir,
            state_file,
            hooks_slot,
        })
    }

    /// Path of the state file the hook writes into.
    #[cfg(test)]
    pub(crate) fn state_file(&self) -> &Path {
        &self.state_file
    }

    /// Environment variable name / value pair the caller should set on
    /// the spawned `codex` process.
    pub(crate) fn env_var(&self) -> (&'static str, &Path) {
        (ITER_STATE_ENV, &self.state_file)
    }

    /// Read whatever the hook captured, restore the original hooks.json,
    /// and remove every scratch file the install path wrote.
    pub(crate) async fn finalize(self) -> Result<HookCapture, AgentError> {
        let capture = self.read_capture().await?;
        self.hooks_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(capture)
    }

    async fn read_capture(&self) -> Result<HookCapture, AgentError> {
        let state_bytes = match fs::read(&self.state_file).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HookCapture::default());
            }
            Err(e) => return Err(map_hook_io("read codex iter-state.json")(e)),
        };

        let payload: Value = serde_json::from_slice(&state_bytes).map_err(|e| {
            AgentError::HookStateParse(format!("decode codex iter-state.json: {e}"))
        })?;

        // Some Codex builds pass the last assistant text directly in the
        // Stop payload under `last_assistant_message`; others only supply
        // `transcript_path`. Accept either so the hook is resilient to
        // the feature-flagged divergence across Codex releases.
        if let Some(direct) = payload
            .get("last_assistant_message")
            .and_then(Value::as_str)
        {
            return Ok(HookCapture {
                last_output: Some(direct.to_owned()),
                turn_count: Some(1),
            });
        }

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

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.codex_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove codex hook script").await?;

        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await);
        }

        remove_if_exists(&self.state_file, "remove codex iter-state.json").await?;

        let bundle_dir = self.codex_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove codex .iter-bundle directory")(e)),
        }

        drop(fs::remove_dir(&self.codex_dir).await);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, bytes).await.expect("write");
    }

    #[tokio::test]
    async fn install_creates_hooks_json_and_script() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        let hooks = tmp.path().join(".codex/hooks.json");
        assert!(hooks.exists());
        let parsed: Value =
            serde_json::from_slice(&fs::read(&hooks).await.expect("read")).expect("json");
        let hook_cmd = parsed["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            hook_cmd.ends_with("codex-loop-hook.sh"),
            "hook command must point at codex-loop-hook.sh, got {hook_cmd}"
        );
        assert_eq!(parsed["hooks"]["Stop"][0]["hooks"][0]["timeout"], 30);

        let script = tmp.path().join(".codex/hooks/codex-loop-hook.sh");
        assert!(script.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&script)
                .await
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "script must be executable");
        }

        assert_eq!(
            bundle.state_file(),
            tmp.path().join(".codex/iter-state.json")
        );
        let (env_key, env_val) = bundle.env_var();
        assert_eq!(env_key, "ITER_STATE_FILE");
        assert_eq!(env_val, bundle.state_file());
    }

    #[tokio::test]
    async fn install_backs_up_pre_existing_hooks_json() {
        let tmp = TempDir::new().expect("tmp");
        let hooks_path = tmp.path().join(".codex/hooks.json");
        let original = json!({ "user_authored": true });
        write_file(
            &hooks_path,
            serde_json::to_vec_pretty(&original).unwrap().as_slice(),
        )
        .await;

        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        assert!(
            tmp.path()
                .join(".codex/.iter-bundle/hooks.json.bak")
                .exists(),
            "backup file must exist"
        );
        bundle.finalize().await.expect("finalize");

        let restored: Value =
            serde_json::from_slice(&fs::read(&hooks_path).await.expect("read")).expect("json");
        assert_eq!(restored, original);
        assert!(!tmp.path().join(".codex/.iter-bundle").exists());
    }

    #[tokio::test]
    async fn finalize_deletes_synthesized_hooks_json_when_none_existed() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        assert!(tmp.path().join(".codex/hooks.json").exists());
        bundle.finalize().await.expect("finalize");
        assert!(!tmp.path().join(".codex/hooks.json").exists());
    }

    #[tokio::test]
    async fn finalize_prefers_last_assistant_message_when_present() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        let state_payload = json!({
            "session_id": "s",
            "last_assistant_message": "direct final",
            "stop_hook_active": false,
        });
        fs::write(
            bundle.state_file(),
            serde_json::to_vec(&state_payload).unwrap(),
        )
        .await
        .expect("write state");
        let capture = bundle.finalize().await.expect("finalize");
        assert_eq!(capture.last_output.as_deref(), Some("direct final"));
        assert_eq!(capture.turn_count, Some(1));
    }

    #[tokio::test]
    async fn finalize_falls_back_to_transcript_path() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        let transcript = tmp.path().join("transcript.jsonl");
        let line = json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "from-transcript"}]},
        });
        fs::write(&transcript, format!("{line}\n"))
            .await
            .expect("write transcript");
        let state_payload = json!({
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
        assert_eq!(capture.last_output.as_deref(), Some("from-transcript"));
        assert_eq!(capture.turn_count, Some(1));
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
    async fn finalize_rejects_malformed_state_file() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        fs::write(bundle.state_file(), b"not json")
            .await
            .expect("write");
        let err = bundle.finalize().await.expect_err("must reject");
        assert!(matches!(err, AgentError::HookStateParse(_)));
    }
}
