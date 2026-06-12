//! Project-local `AfterAgent` hook for
//! [`GeminiAgent`](crate::agent::GeminiAgent)'s interactive/TUI mode.
//!
//! The stop hook exists for **one reason**: to terminate the Gemini CLI
//! interactive session after the agent finishes its task. In print mode
//! the CLI auto-exits — **no hook is needed**.
//!
//! Gemini uses the `AfterAgent` hook event (not `Stop`). The hook's job
//! is:
//!
//! 1. Run any pre-existing user `AfterAgent` hook commands first.
//! 2. Send SIGKILL to the Gemini CLI process.
//!
//! # PID resolution
//!
//! Gemini's `AfterAgent` hook runs as a child of the `gemini` process
//! that iter spawned. `$PPID` is the Gemini CLI PID. SIGKILL because
//! the agent has finished; the TUI is just waiting for human input.
//!
//! # Stop-hook installation files
//!
//! Hook state lives under
//! `~/.iter/projects/<workspace-id>/<isolation-key>/hooks/`,
//! never inside the workspace.
//!
//! # Config file layout
//!
//! The installed hook writes to `${cwd}/.gemini/settings.json`. The
//! previous content is backed up via [`BackupSlot`] and restored on
//! finalize.

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_install::{
    BackupSlot, extract_user_hooks, make_executable, map_hook_io, remove_if_exists,
    shell_single_quote, workspace_hooks_dir,
};

const GEMINI_DIR: &str = ".gemini";
const SETTINGS_FILE: &str = "settings.json";
const HOOK_SCRIPT_REL: &str = "hooks/gemini-loop-hook.sh";
const BUNDLE_DIR: &str = ".iter-bundle";
const SETTINGS_BACKUP_NAME: &str = "settings.json.bak";

fn hook_script_body(user_hooks_script: Option<&Path>) -> String {
    use std::fmt::Write;
    let mut script = String::from(
        "#!/usr/bin/env bash\n\
         # iter AfterAgent hook for Gemini — installed by iter_core::agent::GeminiAgent.\n\
         #\n\
         # Terminates the Gemini TUI session after the agent finishes its task.\n\
         # Runs any pre-existing user AfterAgent hook commands first, then\n\
         # sends SIGKILL to $PPID (the Gemini CLI process).\n\
         set -euo pipefail\n",
    );

    if let Some(path) = user_hooks_script {
        let quoted = shell_single_quote(&path.display().to_string());
        let _ = write!(
            script,
            "\n# Run user's pre-existing AfterAgent hook commands.\n\
             if [[ -x {quoted} ]]; then\n\
             \t{quoted} || true\n\
             fi\n",
        );
    }

    script.push_str(
        "\n# Drain stdin (AfterAgent event payload) so the pipe does not block.\n\
         cat > /dev/null\n\
         \n\
         # SIGKILL the Gemini TUI. $PPID is the gemini process that spawned\n\
         # this hook.\n\
         if [[ -n \"${PPID:-}\" ]]; then\n\
         \tkill -KILL \"$PPID\" 2>/dev/null || true\n\
         fi\n",
    );

    script
}

/// Handle returned from [`HookBundle::install`].
#[derive(Debug)]
pub(crate) struct HookBundle {
    gemini_dir: PathBuf,
    settings_slot: BackupSlot,
}

impl HookBundle {
    /// Install the workspace-local `AfterAgent` hook bundle under `cwd`.
    ///
    /// `isolation_key` is the per-exploration hook isolation key.
    pub(crate) async fn install(cwd: &Path, isolation_key: &str) -> Result<Self, AgentError> {
        let gemini_dir = cwd.join(GEMINI_DIR);
        let bundle_dir = gemini_dir.join(BUNDLE_DIR);
        let settings_path = gemini_dir.join(SETTINGS_FILE);
        let hook_script = gemini_dir.join(HOOK_SCRIPT_REL);

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

        let hooks_dir = workspace_hooks_dir(cwd, isolation_key)?;
        let user_hooks_script =
            extract_user_hooks(&settings_path, "AfterAgent", &hooks_dir).await?;

        let hook_cmd = hook_script
            .to_str()
            .ok_or_else(|| {
                AgentError::Launch(format!(
                    "hook script path is not valid UTF-8: {}",
                    hook_script.display()
                ))
            })?
            .to_owned();
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
            .map_err(|e| AgentError::Launch(format!("serialize gemini settings.json: {e}")))?;
        fs::write(&settings_path, settings_bytes)
            .await
            .map_err(map_hook_io("write synthesized gemini settings.json"))?;

        let body = hook_script_body(user_hooks_script.as_deref());
        fs::write(&hook_script, body.as_bytes())
            .await
            .map_err(map_hook_io("write gemini hook script"))?;
        make_executable(&hook_script).await?;

        Ok(Self {
            gemini_dir,
            settings_slot,
        })
    }

    /// Restore the original settings and remove scratch files.
    pub(crate) async fn finalize(self) -> Result<(), AgentError> {
        self.settings_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(())
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.gemini_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove gemini hook script").await?;

        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await);
        }

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
    use serde_json::{Value, json};
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
        let _bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");

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
        let body = fs::read_to_string(&script).await.expect("read");
        assert!(body.contains("kill -KILL \"$PPID\""));
        assert!(!body.contains("iter-state"));

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

        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
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
    async fn finalize_deletes_synthesized_settings_when_none_existed() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
        assert!(tmp.path().join(".gemini/settings.json").exists());
        bundle.finalize().await.expect("finalize");
        assert!(!tmp.path().join(".gemini/settings.json").exists());
    }
}
