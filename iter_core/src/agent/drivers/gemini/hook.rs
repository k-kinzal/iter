//! Project-local `AfterAgent` hook plumbing for
//! [`GeminiAgent`](crate::agent::GeminiAgent)'s interactive mode.
//!
//! iter's interactive Gemini mode is a descendant of
//! [`agent-loop/gemini-loop`](https://github.com/k-kinzal/agent-loop).
//! Gemini's hook protocol is notably simpler than Claude's or Codex's:
//! the `AfterAgent` hook receives the final agent response directly on
//! stdin as a `prompt_response` field — there is no separate transcript
//! JSONL to parse. That collapses [`HookBundle::finalize`] down to a
//! single JSON decode of the state file.
//!
//! The key invariant — identical to every other hook module in this
//! crate — is that **everything lives under `${cwd}/.gemini/`**, never
//! under `~/.gemini/`. Writing to the global config would silently
//! affect every other Gemini CLI session on the machine; instead we
//! mutate the project-local settings, back up whatever was already
//! there, and restore it when the run ends.
//!
//! # Layout
//!
//! ```text
//! ${cwd}/.gemini/
//!   settings.json                 # overwritten, previous content backed up
//!   hooks/gemini-loop-hook.sh     # the AfterAgent hook body (executable)
//!   iter-state.json               # AfterAgent event payload captured from stdin
//!   .iter-bundle/
//!     settings.json.bak           # backup of any pre-existing settings.json
//! ```

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, HookCapture, ITER_STATE_ENV, make_executable, map_hook_io, remove_if_exists,
};

/// Relative layout under `${cwd}/.gemini/`.
const GEMINI_DIR: &str = ".gemini";
const SETTINGS_FILE: &str = "settings.json";
const HOOK_SCRIPT_REL: &str = "hooks/gemini-loop-hook.sh";
const STATE_FILE_REL: &str = "iter-state.json";
const BUNDLE_DIR: &str = ".iter-bundle";
const SETTINGS_BACKUP_NAME: &str = "settings.json.bak";

/// Bash source of the Gemini `AfterAgent` hook. The `AfterAgent` hook is
/// called once when the agent finishes its run, with the response body
/// on stdin; we slurp stdin verbatim and print a `{"continue": false}`
/// continuation response. Rust reads `prompt_response` out of the state
/// file during finalize.
const HOOK_SCRIPT_BODY: &str = r#"#!/usr/bin/env bash
# iter AfterAgent hook for Gemini — installed by iter_core::agent::GeminiAgent.
# Writes the AfterAgent event payload (stdin JSON) into $ITER_STATE_FILE
# and tells Gemini to exit by printing {"continue": false}.
set -euo pipefail

STATE_FILE="${ITER_STATE_FILE:-$PWD/.gemini/iter-state.json}"
mkdir -p "$(dirname "$STATE_FILE")"
cat > "$STATE_FILE"
printf '%s\n' '{"continue": false, "stopReason": "iter gemini hook captured"}'
"#;

/// Handle returned from [`HookBundle::install`]. Dropping it does **not**
/// clean up the filesystem — callers must invoke [`HookBundle::finalize`]
/// explicitly so restore errors can surface to the runner.
#[derive(Debug)]
pub(crate) struct HookBundle {
    /// Absolute path to `${cwd}/.gemini/`.
    gemini_dir: PathBuf,
    /// Absolute path to the state file the hook writes into.
    state_file: PathBuf,
    /// Backup slot for the user's pre-existing `settings.json`, if any.
    settings_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local `AfterAgent` hook bundle under `cwd`.
    pub(crate) async fn install(cwd: &Path) -> Result<Self, AgentError> {
        let gemini_dir = cwd.join(GEMINI_DIR);
        let bundle_dir = gemini_dir.join(BUNDLE_DIR);
        let settings_path = gemini_dir.join(SETTINGS_FILE);
        let hook_script = gemini_dir.join(HOOK_SCRIPT_REL);
        let state_file = gemini_dir.join(STATE_FILE_REL);

        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create gemini .iter-bundle directory"))?;
        if let Some(parent) = hook_script.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(map_hook_io("create gemini hooks directory"))?;
        }

        let settings_slot =
            BackupSlot::new(&bundle_dir, settings_path.clone(), SETTINGS_BACKUP_NAME);
        settings_slot.snapshot().await?;

        let hook_cmd = hook_script
            .to_str()
            .ok_or_else(|| {
                AgentError::HookSetup(format!(
                    "hook script path is not valid UTF-8: {}",
                    hook_script.display()
                ))
            })?
            .to_owned();
        // Gemini's AfterAgent hook schema: a list of matcher groups, each
        // with a list of command hook entries. `matcher: "*"` matches
        // every event; `name: "iter"` makes the hook self-identifying in
        // Gemini's logs so users can tell iter from their own hooks.
        let settings_payload = json!({
            "hooks": {
                "AfterAgent": [
                    {
                        "matcher": "*",
                        "hooks": [
                            {
                                "name": "iter",
                                "type": "command",
                                "command": hook_cmd
                            }
                        ]
                    }
                ]
            }
        });
        let settings_bytes = serde_json::to_vec_pretty(&settings_payload)
            .map_err(|e| AgentError::HookSetup(format!("serialize gemini settings.json: {e}")))?;
        fs::write(&settings_path, settings_bytes)
            .await
            .map_err(map_hook_io("write synthesized gemini settings.json"))?;

        fs::write(&hook_script, HOOK_SCRIPT_BODY)
            .await
            .map_err(map_hook_io("write gemini hook script"))?;
        make_executable(&hook_script).await?;

        if let Err(e) = fs::remove_file(&state_file).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(map_hook_io("clear stale gemini iter-state.json")(e));
        }

        Ok(Self {
            gemini_dir,
            state_file,
            settings_slot,
        })
    }

    /// Path of the state file the hook writes into.
    #[cfg(test)]
    pub(crate) fn state_file(&self) -> &Path {
        &self.state_file
    }

    /// Environment variable name / value pair the caller should set on
    /// the spawned `gemini` process.
    pub(crate) fn env_var(&self) -> (&'static str, &Path) {
        (ITER_STATE_ENV, &self.state_file)
    }

    /// Read whatever the hook captured, restore the original settings,
    /// and remove every scratch file the install path wrote.
    pub(crate) async fn finalize(self) -> Result<HookCapture, AgentError> {
        let capture = self.read_capture().await?;
        self.settings_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(capture)
    }

    /// Read `prompt_response` out of the state file. Unlike Claude/Codex
    /// there is no transcript JSONL — the response is delivered directly
    /// in the hook payload, so there is no second-stage parse.
    async fn read_capture(&self) -> Result<HookCapture, AgentError> {
        let state_bytes = match fs::read(&self.state_file).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HookCapture::default());
            }
            Err(e) => return Err(map_hook_io("read gemini iter-state.json")(e)),
        };

        let payload: Value = serde_json::from_slice(&state_bytes).map_err(|e| {
            AgentError::HookStateParse(format!("decode gemini iter-state.json: {e}"))
        })?;

        let response = payload
            .get("prompt_response")
            .and_then(Value::as_str)
            .map(str::to_owned);

        Ok(HookCapture {
            last_output: response,
            turn_count: Some(1),
        })
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.gemini_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove gemini hook script").await?;

        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await);
        }

        remove_if_exists(&self.state_file, "remove gemini iter-state.json").await?;

        let bundle_dir = self.gemini_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove gemini .iter-bundle directory")(e)),
        }

        drop(fs::remove_dir(&self.gemini_dir).await);

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
    async fn install_creates_settings_and_script_and_points_at_hook() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        let settings = tmp.path().join(".gemini/settings.json");
        let parsed: Value =
            serde_json::from_slice(&fs::read(&settings).await.expect("read")).expect("json");
        let hook_cmd = parsed["hooks"]["AfterAgent"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            hook_cmd.ends_with("gemini-loop-hook.sh"),
            "hook command must point at gemini-loop-hook.sh, got {hook_cmd}"
        );
        assert_eq!(
            parsed["hooks"]["AfterAgent"][0]["matcher"]
                .as_str()
                .expect("matcher"),
            "*"
        );

        let script = tmp.path().join(".gemini/hooks/gemini-loop-hook.sh");
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
        let (env_key, env_val) = bundle.env_var();
        assert_eq!(env_key, "ITER_STATE_FILE");
        assert_eq!(env_val, tmp.path().join(".gemini/iter-state.json"));
    }

    #[tokio::test]
    async fn install_backs_up_pre_existing_settings_and_finalize_restores_it() {
        let tmp = TempDir::new().expect("tmp");
        let settings_path = tmp.path().join(".gemini/settings.json");
        let original = json!({ "theme": "light", "mine": true });
        write_file(
            &settings_path,
            serde_json::to_vec_pretty(&original).unwrap().as_slice(),
        )
        .await;

        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        assert!(
            tmp.path()
                .join(".gemini/.iter-bundle/settings.json.bak")
                .exists()
        );
        bundle.finalize().await.expect("finalize");
        let restored: Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(restored, original);
        assert!(!tmp.path().join(".gemini/.iter-bundle").exists());
    }

    #[tokio::test]
    async fn finalize_extracts_prompt_response() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        let state = json!({ "prompt_response": "final gemini answer" });
        fs::write(bundle.state_file(), serde_json::to_vec(&state).unwrap())
            .await
            .expect("write");
        let capture = bundle.finalize().await.expect("finalize");
        assert_eq!(capture.last_output.as_deref(), Some("final gemini answer"));
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
        fs::write(bundle.state_file(), b"<not json>")
            .await
            .expect("write");
        let err = bundle.finalize().await.expect_err("must reject");
        assert!(matches!(err, AgentError::HookStateParse(_)));
    }
}
