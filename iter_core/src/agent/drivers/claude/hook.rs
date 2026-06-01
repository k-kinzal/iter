//! Project-local Stop hook for [`ClaudeAgent`](crate::agent::ClaudeAgent)'s
//! interactive/TUI mode.
//!
//! The stop hook exists for **one reason**: to terminate the Claude Code
//! interactive TUI session after the agent finishes its task. In print
//! mode (`claude --print`) the CLI auto-exits when it finishes
//! responding — **no hook is needed**.
//!
//! In interactive/TUI mode, the CLI stays open after a turn completes
//! (waiting for the next human input). Without external intervention,
//! iter's iteration cannot proceed because the agent process never exits.
//!
//! The hook's job in TUI mode is:
//!
//! 1. If the user has registered their own Stop Hook commands in
//!    `.claude/settings.json`, **run those first** (preserve user
//!    behavior — iter's hook must not displace it).
//! 2. Once those have run, send SIGKILL to the Claude Code process so
//!    the TUI session terminates and iter regains control.
//!
//! # PID resolution
//!
//! Claude Code's Stop hook runs as a child of the `claude` process that
//! iter spawned. Inside the hook script, `$PPID` is the PID of the
//! `claude` CLI process — the exact process that needs to be killed.
//! SIGKILL (not SIGTERM) because the agent has already finished its
//! task — the TUI is simply waiting for the next human input; there is
//! nothing to gracefully shut down.
//!
//! # Sidecar files
//!
//! All per-service hook state lives under
//! `~/.iter/projects/<project-id>/<service>/hooks/`, never inside the
//! workspace. This ensures iter does not pollute the project tree.
//!
//! - `existing-stop-hooks.sh` — user's pre-existing Stop commands
//!   extracted on install, executed by the hook before SIGKILL.
//!
//! # Config file layout
//!
//! The installed hook writes to `${cwd}/.claude/settings.json`. The
//! previous content is backed up via [`BackupSlot`] and restored on
//! finalize.

use std::path::{Path, PathBuf};

use serde_json::json;
use tokio::fs;

use crate::agent::AgentError;
use crate::agent::hook_lifecycle::{
    BackupSlot, extract_user_hooks, make_executable, map_hook_io, project_hooks_dir,
    remove_if_exists, shell_single_quote,
};

const CLAUDE_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const HOOK_SCRIPT_REL: &str = "hooks/iter-stop-hook.sh";
const BUNDLE_DIR: &str = ".iter-bundle";
const SETTINGS_BACKUP_NAME: &str = "settings.json.bak";

/// Build the bash hook script body. If `user_hooks_sidecar` is `Some`,
/// the script sources it before killing the agent.
fn hook_script_body(user_hooks_sidecar: Option<&Path>) -> String {
    use std::fmt::Write;
    let mut script = String::from(
        "#!/usr/bin/env bash\n\
         # iter Stop hook for Claude Code — installed by iter_core::agent::ClaudeAgent.\n\
         #\n\
         # Terminates the Claude Code TUI session after the agent finishes its\n\
         # task. Runs any pre-existing user Stop Hook commands first, then sends\n\
         # SIGKILL to $PPID (the Claude CLI process).\n\
         set -euo pipefail\n",
    );

    if let Some(sidecar) = user_hooks_sidecar {
        let quoted = shell_single_quote(&sidecar.display().to_string());
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
         # SIGKILL the Claude Code TUI. $PPID is the claude process that\n\
         # spawned this hook. SIGKILL because the agent has finished — the TUI\n\
         # is just waiting for human input.\n\
         if [[ -n \"${PPID:-}\" ]]; then\n\
         \tkill -KILL \"$PPID\" 2>/dev/null || true\n\
         fi\n",
    );

    script
}

/// Handle returned from [`HookBundle::install`]. Dropping it does **not**
/// clean up the filesystem — callers must invoke [`HookBundle::finalize`]
/// explicitly so that restore errors can surface to the runner.
#[derive(Debug)]
pub(crate) struct HookBundle {
    claude_dir: PathBuf,
    settings_slot: BackupSlot,
}

impl HookBundle {
    /// Install the project-local Stop hook bundle under `cwd`.
    ///
    /// Creates `${cwd}/.claude/` and its required subdirectories, backs up
    /// any pre-existing `settings.json`, extracts user Stop hooks into a
    /// sidecar under `~/.iter/projects/`, writes a fresh `settings.json`
    /// pointing at the iter hook script, and writes the script body with
    /// mode `0o755`.
    ///
    /// `service` is the compose service name or `"default"` for standalone
    /// `iter run`.
    pub(crate) async fn install(cwd: &Path, service: &str) -> Result<Self, AgentError> {
        let claude_dir = cwd.join(CLAUDE_DIR);
        let bundle_dir = claude_dir.join(BUNDLE_DIR);
        let settings_path = claude_dir.join(SETTINGS_FILE);
        let hook_script = claude_dir.join(HOOK_SCRIPT_REL);

        fs::create_dir_all(&bundle_dir)
            .await
            .map_err(map_hook_io("create .iter-bundle directory"))?;
        if let Some(parent) = hook_script.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(map_hook_io("create hooks directory"))?;
        }

        let settings_slot =
            BackupSlot::new(&bundle_dir, settings_path.clone(), SETTINGS_BACKUP_NAME);
        settings_slot.snapshot().await?;

        // Extract any pre-existing user Stop hooks into a sidecar.
        let hooks_dir = project_hooks_dir(cwd, service)?;
        let user_hooks_sidecar =
            extract_user_hooks(&settings_path, "Stop", &hooks_dir).await?;

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
            .map_err(|e| AgentError::Launch(format!("serialize settings.json: {e}")))?;
        fs::write(&settings_path, settings_bytes)
            .await
            .map_err(map_hook_io("write synthesized settings.json"))?;

        let body = hook_script_body(user_hooks_sidecar.as_deref());
        fs::write(&hook_script, body.as_bytes())
            .await
            .map_err(map_hook_io("write hook script"))?;
        make_executable(&hook_script).await?;

        Ok(Self {
            claude_dir,
            settings_slot,
        })
    }

    /// Restore the original settings file and remove every scratch file
    /// the install path wrote.
    pub(crate) async fn finalize(self) -> Result<(), AgentError> {
        self.settings_slot.restore().await?;
        self.cleanup_scratch().await?;
        Ok(())
    }

    async fn cleanup_scratch(&self) -> Result<(), AgentError> {
        let hook_script = self.claude_dir.join(HOOK_SCRIPT_REL);
        remove_if_exists(&hook_script, "remove hook script").await?;

        if let Some(parent) = hook_script.parent() {
            drop(fs::remove_dir(parent).await);
        }

        let bundle_dir = self.claude_dir.join(BUNDLE_DIR);
        match fs::remove_dir_all(&bundle_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_hook_io("remove .iter-bundle directory")(e)),
        }

        drop(fs::remove_dir(&self.claude_dir).await);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    async fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, bytes).await.expect("write");
    }

    #[tokio::test]
    async fn install_creates_settings_and_hook_script() {
        let tmp = TempDir::new().expect("tmp");
        let _bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");

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
        let body = fs::read_to_string(&script).await.expect("read script");
        assert!(
            body.contains("kill -KILL \"$PPID\""),
            "hook script must SIGKILL $PPID"
        );
        assert!(
            !body.contains("iter-state"),
            "hook script must not reference state file"
        );
        assert!(
            !body.contains("continue"),
            "hook script must not emit JSON continuation response"
        );

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

        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");

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
        let bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
        assert!(tmp.path().join(".claude/settings.json").exists());
        bundle.finalize().await.expect("finalize");
        assert!(
            !tmp.path().join(".claude/settings.json").exists(),
            "synthesized settings.json must be removed when no backup existed"
        );
    }

    #[tokio::test]
    async fn hook_script_drains_stdin() {
        let tmp = TempDir::new().expect("tmp");
        let _bundle = HookBundle::install(tmp.path(), "default")
            .await
            .expect("install");
        let script = tmp.path().join(".claude/hooks/iter-stop-hook.sh");
        let body = fs::read_to_string(&script).await.expect("read");
        assert!(
            body.contains("cat > /dev/null"),
            "hook script must drain stdin"
        );
    }

    #[tokio::test]
    async fn hook_script_includes_user_hooks_when_present() {
        let tmp = TempDir::new().expect("tmp");
        let settings_path = tmp.path().join(".claude/settings.json");
        let user_config = json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo my-user-hook" }
                        ]
                    }
                ]
            }
        });
        write(
            &settings_path,
            serde_json::to_vec_pretty(&user_config).unwrap().as_slice(),
        )
        .await;

        let _bundle = HookBundle::install(tmp.path(), "test_svc")
            .await
            .expect("install");

        let script = tmp.path().join(".claude/hooks/iter-stop-hook.sh");
        let body = fs::read_to_string(&script).await.expect("read");
        assert!(
            body.contains("existing-stop-hooks.sh"),
            "hook script must reference user hooks sidecar"
        );
    }
}
