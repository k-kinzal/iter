//! Project-local agentStop hook plumbing for
//! [`CopilotAgent`](crate::agent::CopilotAgent)'s interactive mode.
//!
//! iter's interactive Copilot mode is a descendant of
//! [`agent-loop/copilot-loop`](https://github.com/k-kinzal/agent-loop).
//! Copilot's hook protocol differs from the other three hook-based
//! agents in two important ways:
//!
//! 1. **Two-file bundle.** The hook bundle consists of *two* files under
//!    `${cwd}/.github/hooks/`: a `copilot-loop.json` configuration that
//!    registers the hook and a `copilot-loop-hook.sh` bash body. Both
//!    are backed up + restored via their own [`BackupSlot`].
//! 2. **Different transcript schema.** Where Claude/Codex transcripts
//!    use `{"type":"assistant","message":{"content":[...]}}`, Copilot
//!    uses `{"type":"assistant.message","data":{"content":"..."}}`.
//!    Parsing is handled by
//!    [`parse_copilot_transcript`](crate::agent::transcript::parse_copilot_transcript).
//!
//! The key invariant — identical to every other hook module in this
//! crate — is that **everything lives under `${cwd}/.github/hooks/`**,
//! never under `~/.github/` or the user's home directory. Writing to the
//! global config would silently affect every other Copilot session on
//! the machine.
//!
//! # Stop signalling
//!
//! agent-loop's Copilot hook kills both its parent and its grandparent
//! with SIGTERM to force the Copilot TUI out of its read loop. For iter
//! this is almost correct but not exactly: iter's process tree is
//!
//! ```text
//! iter runner                 (PPID of Copilot; must NOT be killed)
//!   └── copilot CLI child      (parent of the hook script; SIGTERM here)
//!         └── hook.sh          (gets SIGTERM from kill -TERM $PPID)
//! ```
//!
//! so our hook script only sends `SIGTERM` to `$PPID`. Killing the
//! grandparent (iter itself) would abort the runner mid-iteration, which
//! is catastrophic.
//!
//! # Layout
//!
//! ```text
//! ${cwd}/.github/hooks/
//!   copilot-loop.json                 # hook config (AgentStop handler)
//!   copilot-loop-hook.sh              # hook body (executable)
//!   iter-state.json                   # AgentStop payload captured from stdin
//!   .iter-bundle/
//!     copilot-loop.json.bak           # backup of any pre-existing config
//!     copilot-loop-hook.sh.bak        # backup of any pre-existing script
//! ```

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, HookCapture, ITER_STATE_ENV, make_executable, map_hook_io, remove_if_exists,
};
use crate::agent::transcript::parse_copilot_transcript;

/// Relative layout under `${cwd}/.github/hooks/`.
const HOOKS_DIR: &str = ".github/hooks";
const HOOK_CONFIG_FILE: &str = "copilot-loop.json";
const HOOK_SCRIPT_FILE: &str = "copilot-loop-hook.sh";
const STATE_FILE: &str = "iter-state.json";
const BUNDLE_DIR: &str = ".iter-bundle";
const CONFIG_BACKUP_NAME: &str = "copilot-loop.json.bak";
const SCRIPT_BACKUP_NAME: &str = "copilot-loop-hook.sh.bak";

/// Bash source of the Copilot agentStop hook. Slurps stdin into the
/// state file and SIGTERMs its parent (the Copilot CLI iter spawned).
///
/// Unlike agent-loop this script does NOT kill its grandparent. In iter
/// the grandparent is the runner process, and the runner must stay
/// alive to handle the next signal.
const HOOK_SCRIPT_BODY: &str = r#"#!/usr/bin/env bash
# iter agentStop hook for Copilot — installed by iter_core::agent::CopilotAgent.
# Writes the agentStop event payload (stdin JSON) into $ITER_STATE_FILE
# and SIGTERMs the Copilot CLI parent process so it exits promptly.
#
# Unlike agent-loop's copilot-loop.sh this script NEVER kills its
# grandparent — iter's runner lives there and must not be disturbed.
set -euo pipefail

STATE_FILE="${ITER_STATE_FILE:-$PWD/.github/hooks/iter-state.json}"
mkdir -p "$(dirname "$STATE_FILE")"
cat > "$STATE_FILE"

if [[ -n "${PPID:-}" ]]; then
    kill -TERM "$PPID" 2>/dev/null || true
fi
"#;

/// Handle returned from [`HookBundle::install`]. Dropping it does **not**
/// clean up the filesystem — callers must invoke [`HookBundle::finalize`]
/// explicitly so restore errors can surface to the runner.
#[derive(Debug)]
pub(crate) struct HookBundle {
    /// Absolute path to `${cwd}/.github/hooks/`.
    hooks_dir: PathBuf,
    /// Absolute path to the state file the hook writes into.
    state_file: PathBuf,
    /// Backup slot for the user's pre-existing `copilot-loop.json`, if any.
    config_slot: BackupSlot,
    /// Backup slot for the user's pre-existing `copilot-loop-hook.sh`, if any.
    script_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local agentStop hook bundle under `cwd`.
    pub(crate) async fn install(cwd: &Path) -> Result<Self, AgentError> {
        let hooks_dir = cwd.join(HOOKS_DIR);
        let bundle_dir = hooks_dir.join(BUNDLE_DIR);
        let config_path = hooks_dir.join(HOOK_CONFIG_FILE);
        let script_path = hooks_dir.join(HOOK_SCRIPT_FILE);
        let state_file = hooks_dir.join(STATE_FILE);

        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create copilot .iter-bundle directory"))?;

        let config_slot = BackupSlot::new(&bundle_dir, config_path.clone(), CONFIG_BACKUP_NAME);
        config_slot.snapshot().await?;
        let script_slot = BackupSlot::new(&bundle_dir, script_path.clone(), SCRIPT_BACKUP_NAME);
        script_slot.snapshot().await?;

        // Copilot's agentStop hook config format. `bash` is a relative
        // path (from the Copilot CLI's working directory, which iter
        // sets to `cwd`) matching the agent-loop convention. The
        // `version: 1` field is required by Copilot's hook loader.
        let config_payload = json!({
            "version": 1,
            "hooks": {
                "agentStop": [
                    {
                        "type": "command",
                        "bash": format!("./{HOOKS_DIR}/{HOOK_SCRIPT_FILE}")
                    }
                ]
            }
        });
        let config_bytes = serde_json::to_vec_pretty(&config_payload)
            .map_err(|e| AgentError::HookSetup(format!("serialize copilot-loop.json: {e}")))?;
        fs::write(&config_path, config_bytes)
            .await
            .map_err(map_hook_io("write synthesized copilot-loop.json"))?;

        fs::write(&script_path, HOOK_SCRIPT_BODY)
            .await
            .map_err(map_hook_io("write copilot hook script"))?;
        make_executable(&script_path).await?;

        if let Err(e) = fs::remove_file(&state_file).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(map_hook_io("clear stale copilot iter-state.json")(e));
        }

        Ok(Self {
            hooks_dir,
            state_file,
            config_slot,
            script_slot,
        })
    }

    /// Path of the state file the hook writes into.
    #[cfg(test)]
    pub(crate) fn state_file(&self) -> &Path {
        &self.state_file
    }

    /// Environment variable name / value pair the caller should set on
    /// the spawned Copilot process.
    pub(crate) fn env_var(&self) -> (&'static str, &Path) {
        (ITER_STATE_ENV, &self.state_file)
    }

    /// Read whatever the hook captured, restore the original config +
    /// script files, and remove every scratch file the install path
    /// wrote.
    pub(crate) async fn finalize(self) -> Result<HookCapture, AgentError> {
        let capture = self.read_capture().await?;
        self.script_slot.restore().await?;
        self.config_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(capture)
    }

    async fn read_capture(&self) -> Result<HookCapture, AgentError> {
        let state_bytes = match fs::read(&self.state_file).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HookCapture::default());
            }
            Err(e) => return Err(map_hook_io("read copilot iter-state.json")(e)),
        };

        let payload: Value = serde_json::from_slice(&state_bytes).map_err(|e| {
            AgentError::HookStateParse(format!("decode copilot iter-state.json: {e}"))
        })?;

        // Copilot's agentStop payload carries `transcriptPath` (camel
        // case) pointing at a JSONL file with `assistant.message`
        // events. Some builds instead embed `last_assistant_message`
        // directly; accept either so the module survives protocol
        // drift.
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
            .get("transcriptPath")
            .or_else(|| payload.get("transcript_path"))
            .and_then(Value::as_str)
            .map(PathBuf::from);

        let Some(transcript) = transcript_path else {
            return Ok(HookCapture {
                last_output: None,
                turn_count: Some(0),
            });
        };

        parse_copilot_transcript(&transcript).await
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        remove_if_exists(&self.state_file, "remove copilot iter-state.json").await?;

        let bundle_dir = self.hooks_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove copilot .iter-bundle directory")(e)),
        }

        // Best-effort: prune .github/hooks/ if empty (we may have
        // created it). Then .github/ itself if that ended up empty too.
        drop(fs::remove_dir(&self.hooks_dir).await);
        if let Some(dotgithub) = self.hooks_dir.parent() {
            drop(fs::remove_dir(dotgithub).await);
        }

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
    async fn install_creates_config_and_script() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        let config = tmp.path().join(".github/hooks/copilot-loop.json");
        let parsed: Value =
            serde_json::from_slice(&fs::read(&config).await.expect("read")).expect("json");
        assert_eq!(parsed["version"], 1);
        let bash = parsed["hooks"]["agentStop"][0]["bash"]
            .as_str()
            .expect("bash");
        assert!(
            bash.ends_with("copilot-loop-hook.sh"),
            "bash command must point at copilot-loop-hook.sh, got {bash}"
        );

        let script = tmp.path().join(".github/hooks/copilot-loop-hook.sh");
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
        assert_eq!(env_val, tmp.path().join(".github/hooks/iter-state.json"));
    }

    #[tokio::test]
    async fn install_backs_up_pre_existing_config_and_script_and_finalize_restores_both() {
        let tmp = TempDir::new().expect("tmp");
        let config_path = tmp.path().join(".github/hooks/copilot-loop.json");
        let script_path = tmp.path().join(".github/hooks/copilot-loop-hook.sh");
        let original_config = json!({ "user_config": true });
        write_file(
            &config_path,
            serde_json::to_vec_pretty(&original_config)
                .unwrap()
                .as_slice(),
        )
        .await;
        let original_script = b"#!/usr/bin/env bash\necho user script\n";
        write_file(&script_path, original_script).await;

        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        assert!(
            tmp.path()
                .join(".github/hooks/.iter-bundle/copilot-loop.json.bak")
                .exists()
        );
        assert!(
            tmp.path()
                .join(".github/hooks/.iter-bundle/copilot-loop-hook.sh.bak")
                .exists()
        );

        bundle.finalize().await.expect("finalize");
        let restored_config: Value =
            serde_json::from_slice(&fs::read(&config_path).await.expect("read")).expect("json");
        assert_eq!(restored_config, original_config);
        let restored_script = fs::read(&script_path).await.expect("read");
        assert_eq!(restored_script, original_script);
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
    async fn finalize_prefers_last_assistant_message_when_present() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        let state = json!({ "last_assistant_message": "inline answer" });
        fs::write(bundle.state_file(), serde_json::to_vec(&state).unwrap())
            .await
            .expect("write");
        let capture = bundle.finalize().await.expect("finalize");
        assert_eq!(capture.last_output.as_deref(), Some("inline answer"));
        assert_eq!(capture.turn_count, Some(1));
    }

    #[tokio::test]
    async fn finalize_parses_copilot_transcript_via_transcript_path() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");

        let transcript = tmp.path().join("transcript.jsonl");
        let lines = [
            json!({
                "type": "assistant.message",
                "data": {"content": "first"},
            }),
            json!({
                "type": "assistant.message",
                "data": {"content": "second"},
            }),
        ];
        let mut body = String::new();
        for line in &lines {
            body.push_str(&serde_json::to_string(line).unwrap());
            body.push('\n');
        }
        fs::write(&transcript, body).await.expect("write");

        let state = json!({ "transcriptPath": transcript.to_str().unwrap() });
        fs::write(bundle.state_file(), serde_json::to_vec(&state).unwrap())
            .await
            .expect("write state");

        let capture = bundle.finalize().await.expect("finalize");
        assert_eq!(capture.last_output.as_deref(), Some("second"));
        assert_eq!(capture.turn_count, Some(2));
    }

    #[tokio::test]
    async fn finalize_rejects_malformed_state_file() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path()).await.expect("install");
        fs::write(bundle.state_file(), b"</not json/>")
            .await
            .expect("write");
        let err = bundle.finalize().await.expect_err("must reject");
        assert!(matches!(err, AgentError::HookStateParse(_)));
    }
}
