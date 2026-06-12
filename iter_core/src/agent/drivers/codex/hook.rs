//! Project-local Stop hook for [`CodexAgent`](crate::agent::CodexAgent)'s
//! interactive/TUI mode.
//!
//! The stop hook exists for **one reason**: to terminate the Codex
//! interactive TUI session after the agent finishes its task. In print
//! mode the CLI auto-exits — **no hook is needed**.
//!
//! In interactive/TUI mode, the CLI stays open after a turn completes.
//! The hook's job is:
//!
//! 1. Run any pre-existing user Stop Hook commands first.
//! 2. Send SIGKILL to the Codex CLI process.
//!
//! # PID resolution
//!
//! Codex's Stop hook runs as a child of the `codex` process that iter
//! spawned. `$PPID` inside the hook is the Codex CLI PID — the process
//! to kill. SIGKILL because the agent has finished; the TUI is waiting
//! for human input.
//!
//! # Stop-hook installation files
//!
//! Hook state lives under
//! `~/.iter/projects/<workspace-id>/<isolation-key>/hooks/`,
//! never inside the workspace.
//!
//! # Config file layout
//!
//! The installed hook writes to `${cwd}/.codex/hooks.json`. The previous
//! content is backed up via [`BackupSlot`] and restored on finalize.

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_install::{
    BackupSlot, extract_user_hooks, make_executable, map_hook_io, remove_if_exists,
    shell_single_quote, workspace_hooks_dir,
};

const CODEX_DIR: &str = ".codex";
const HOOKS_FILE: &str = "hooks.json";
const HOOK_SCRIPT_REL: &str = "hooks/codex-loop-hook.sh";
const BUNDLE_DIR: &str = ".iter-bundle";
const HOOKS_BACKUP_NAME: &str = "hooks.json.bak";

fn hook_script_body(user_hooks_script: Option<&Path>) -> String {
    use std::fmt::Write;
    let mut script = String::from(
        "#!/usr/bin/env bash\n\
         # iter Stop hook for Codex — installed by iter_core::agent::CodexAgent.\n\
         #\n\
         # Terminates the Codex TUI session after the agent finishes its task.\n\
         # Runs any pre-existing user Stop Hook commands first, then sends\n\
         # SIGKILL to $PPID (the Codex CLI process).\n\
         set -euo pipefail\n",
    );

    if let Some(path) = user_hooks_script {
        let quoted = shell_single_quote(&path.display().to_string());
        let _ = write!(
            script,
            "\n# Run user's pre-existing Stop Hook commands.\n\
             if [[ -x {quoted} ]]; then\n\
             \t{quoted} || true\n\
             fi\n",
        );
    }

    script.push_str(
        "\n# Drain stdin (Stop event payload) so the pipe does not block.\n\
         cat > /dev/null\n\
         \n\
         # SIGKILL the Codex TUI. $PPID is the codex process that spawned\n\
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
    codex_dir: PathBuf,
    hooks_slot: BackupSlot,
}

impl HookBundle {
    /// Install the workspace-local Stop hook bundle under `cwd`.
    ///
    /// `isolation_key` is the per-exploration hook isolation key.
    pub(crate) async fn install(cwd: &Path, isolation_key: &str) -> Result<Self, AgentError> {
        let codex_dir = cwd.join(CODEX_DIR);
        let bundle_dir = codex_dir.join(BUNDLE_DIR);
        let hooks_path = codex_dir.join(HOOKS_FILE);
        let hook_script = codex_dir.join(HOOK_SCRIPT_REL);

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

        let hooks_dir = workspace_hooks_dir(cwd, isolation_key)?;
        let user_hooks_script = extract_user_hooks(&hooks_path, "Stop", &hooks_dir).await?;

        let hook_cmd = hook_script
            .to_str()
            .ok_or_else(|| {
                AgentError::Launch(format!(
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
            .map_err(|e| AgentError::Launch(format!("serialize codex hooks.json: {e}")))?;
        fs::write(&hooks_path, hooks_bytes)
            .await
            .map_err(map_hook_io("write synthesized codex hooks.json"))?;

        let body = hook_script_body(user_hooks_script.as_deref());
        fs::write(&hook_script, body.as_bytes())
            .await
            .map_err(map_hook_io("write codex hook script"))?;
        make_executable(&hook_script).await?;

        Ok(Self {
            codex_dir,
            hooks_slot,
        })
    }

    /// Restore the original hooks.json and remove scratch files.
    pub(crate) async fn finalize(self) -> Result<(), AgentError> {
        self.hooks_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(())
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.codex_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove codex hook script").await?;

        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await);
        }

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
    use serde_json::{Value, json};
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
        let _bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");

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
        let body = fs::read_to_string(&script).await.expect("read");
        assert!(body.contains("kill -KILL \"$PPID\""));
        assert!(!body.contains("iter-state"));
        assert!(!body.contains("continue"));

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
    async fn install_backs_up_pre_existing_hooks_json() {
        let tmp = TempDir::new().expect("tmp");
        let hooks_path = tmp.path().join(".codex/hooks.json");
        let original = json!({ "user_authored": true });
        write_file(
            &hooks_path,
            serde_json::to_vec_pretty(&original).unwrap().as_slice(),
        )
        .await;

        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
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
        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
        assert!(tmp.path().join(".codex/hooks.json").exists());
        bundle.finalize().await.expect("finalize");
        assert!(!tmp.path().join(".codex/hooks.json").exists());
    }
}
