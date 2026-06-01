//! Project-local agentStop hook for
//! [`CopilotAgent`](crate::agent::CopilotAgent)'s interactive/TUI mode.
//!
//! The stop hook exists for **one reason**: to terminate the Copilot
//! interactive TUI session after the agent finishes its task.
//!
//! Copilot's hook protocol differs from the other three in two ways:
//!
//! 1. **Two-file bundle.** The hook consists of a `copilot-loop.json`
//!    config and a `copilot-loop-hook.sh` script, both under
//!    `${cwd}/.github/hooks/`. Both are backed up + restored via their
//!    own [`BackupSlot`].
//! 2. **`agentStop` event name** (not `Stop` or `AfterAgent`).
//!
//! # PID resolution
//!
//! Copilot's agentStop hook runs as a child of the Copilot CLI process.
//! `$PPID` inside the hook is the Copilot CLI PID — the process to kill.
//!
//! iter's process tree is:
//!
//! ```text
//! iter runner                 (grandparent; must NOT be killed)
//!   └── copilot CLI child     (parent of the hook script; SIGKILL here)
//!         └── hook.sh         (kills $PPID = copilot CLI)
//! ```
//!
//! SIGKILL because the agent has finished; the TUI is just waiting for
//! human input.
//!
//! # Sidecar files
//!
//! Hook state lives under `~/.iter/projects/<project-id>/<service>/hooks/`,
//! never inside the workspace.
//!
//! # Config file layout
//!
//! ```text
//! ${cwd}/.github/hooks/
//!   copilot-loop.json          # hook config (agentStop handler)
//!   copilot-loop-hook.sh       # hook body (executable)
//!   .iter-bundle/
//!     copilot-loop.json.bak    # backup of any pre-existing config
//!     copilot-loop-hook.sh.bak # backup of any pre-existing script
//! ```

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, extract_user_hooks, make_executable, map_hook_io, project_hooks_dir,
    shell_single_quote,
};

const HOOKS_DIR: &str = ".github/hooks";
const HOOK_CONFIG_FILE: &str = "copilot-loop.json";
const HOOK_SCRIPT_FILE: &str = "copilot-loop-hook.sh";
const BUNDLE_DIR: &str = ".iter-bundle";
const CONFIG_BACKUP_NAME: &str = "copilot-loop.json.bak";
const SCRIPT_BACKUP_NAME: &str = "copilot-loop-hook.sh.bak";

fn hook_script_body(user_hooks_sidecar: Option<&Path>) -> String {
    use std::fmt::Write;
    let mut script = String::from(
        "#!/usr/bin/env bash\n\
         # iter agentStop hook for Copilot — installed by iter_core::agent::CopilotAgent.\n\
         #\n\
         # Terminates the Copilot TUI session after the agent finishes its task.\n\
         # Runs any pre-existing user agentStop hook commands first, then sends\n\
         # SIGKILL to $PPID (the Copilot CLI process).\n\
         #\n\
         # Unlike agent-loop's copilot-loop.sh this script NEVER kills its\n\
         # grandparent — iter's runner lives there and must not be disturbed.\n\
         set -euo pipefail\n",
    );

    if let Some(sidecar) = user_hooks_sidecar {
        let quoted = shell_single_quote(&sidecar.display().to_string());
        let _ = write!(
            script,
            "\n# Run user's pre-existing agentStop hook commands.\n\
             if [[ -x {quoted} ]]; then\n\
             \t{quoted} || true\n\
             fi\n",
        );
    }

    script.push_str(
        "\n# Drain stdin (agentStop event payload) so the pipe does not block.\n\
         cat > /dev/null\n\
         \n\
         # SIGKILL the Copilot TUI. $PPID is the copilot process that spawned\n\
         # this hook. Never kill grandparent (iter runner).\n\
         if [[ -n \"${PPID:-}\" ]]; then\n\
         \tkill -KILL \"$PPID\" 2>/dev/null || true\n\
         fi\n",
    );

    script
}

/// Handle returned from [`HookBundle::install`].
#[derive(Debug)]
pub(crate) struct HookBundle {
    hooks_dir: PathBuf,
    config_slot: BackupSlot,
    script_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local agentStop hook bundle under `cwd`.
    pub(crate) async fn install(cwd: &Path, service: &str) -> Result<Self, AgentError> {
        let hooks_dir = cwd.join(HOOKS_DIR);
        let bundle_dir = hooks_dir.join(BUNDLE_DIR);
        let config_path = hooks_dir.join(HOOK_CONFIG_FILE);
        let script_path = hooks_dir.join(HOOK_SCRIPT_FILE);

        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create copilot .iter-bundle directory"))?;

        let config_slot = BackupSlot::new(&bundle_dir, config_path.clone(), CONFIG_BACKUP_NAME);
        config_slot.snapshot().await?;
        let script_slot = BackupSlot::new(&bundle_dir, script_path.clone(), SCRIPT_BACKUP_NAME);
        script_slot.snapshot().await?;

        let proj_hooks_dir = project_hooks_dir(cwd, service)?;
        let user_hooks_sidecar =
            extract_user_hooks(&config_path, "agentStop", &proj_hooks_dir).await?;

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
            .map_err(|e| AgentError::Launch(format!("serialize copilot-loop.json: {e}")))?;
        fs::write(&config_path, config_bytes)
            .await
            .map_err(map_hook_io("write synthesized copilot-loop.json"))?;

        let body = hook_script_body(user_hooks_sidecar.as_deref());
        fs::write(&script_path, body.as_bytes())
            .await
            .map_err(map_hook_io("write copilot hook script"))?;
        make_executable(&script_path).await?;

        Ok(Self {
            hooks_dir,
            config_slot,
            script_slot,
        })
    }

    /// Restore the original config + script files and remove scratch files.
    pub(crate) async fn finalize(self) -> Result<(), AgentError> {
        self.script_slot.restore().await?;
        self.config_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(())
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let bundle_dir = self.hooks_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove copilot .iter-bundle directory")(e)),
        }

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
    use serde_json::{Value, json};
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
        let _bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");

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
        let body = fs::read_to_string(&script).await.expect("read");
        assert!(body.contains("kill -KILL \"$PPID\""));
        assert!(!body.contains("iter-state"));
        assert!(body.contains("NEVER kills its"));

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

        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
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
    async fn finalize_deletes_synthesized_files_when_none_existed() {
        let tmp = TempDir::new().expect("tmp");
        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
        assert!(
            tmp.path().join(".github/hooks/copilot-loop.json").exists()
        );
        bundle.finalize().await.expect("finalize");
        assert!(
            !tmp.path().join(".github/hooks/copilot-loop.json").exists()
        );
    }
}
