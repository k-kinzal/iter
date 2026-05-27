//! [`CopilotAgent`] — GitHub Copilot CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   gh copilot suggest [extra-args...] <prompt>
//!   ```
//!
//!   This matches the `gh copilot suggest 'how do I ...'` pattern
//!   documented for `gh-copilot`. The prompt is passed as the final
//!   positional argument.
//!
//! * [`AgentMode::Interactive`] — launches the configured Copilot CLI
//!   binary as a live TUI with a project-local `agentStop` hook
//!   installed under `${cwd}/.github/hooks/`. The hook bundle consists
//!   of **two** files (unlike the other three hook-based agents):
//!   `copilot-loop.json` (the hook config) and `copilot-loop-hook.sh`
//!   (the hook body). Both are backed up and restored.
//!
//!   The hook's sole purpose is to terminate the TUI session — it runs
//!   any pre-existing user agentStop hooks, then sends SIGKILL to the
//!   Copilot CLI process. The hook is a descendant of
//!   [`agent-loop/copilot-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but with one critical divergence: **the hook only kills
//!   its parent (the Copilot CLI), never its grandparent**. In iter the
//!   grandparent is the runner process itself, which must stay alive to
//!   handle the next signal.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.github/hooks/`. iter never writes to the user's
//!   home `.github/` because doing so would silently affect every other
//!   Copilot session on the machine. See
//!   the `hook` submodule for the filesystem layout.
//!
//!   **Binary selection.** In interactive mode, the configured
//!   [`command`](CopilotAgent::command) + [`subcommand`](CopilotAgent::subcommand)
//!   must launch a live TUI that loads `.github/hooks/copilot-loop.json`
//!   on startup. The default (`gh copilot suggest`) is a one-shot print
//!   command and will *not* work in interactive mode; users must point
//!   `command` at the standalone `copilot` TUI binary and clear the
//!   subcommand first:
//!
//!   ```no_run
//!   # use iter_core::agent::{AgentMode, CopilotAgent, CopilotSettings};
//!   let agent = CopilotAgent::new(CopilotSettings {
//!       command: "copilot".into(),
//!       mode: AgentMode::Interactive,
//!       subcommand: Some(Vec::<String>::new()),
//!       args: Vec::new(),
//!       env: Vec::new(),
//!   });
//!   ```
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process so the TUI renders correctly when iter is invoked from a
//!   terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Print`] instead.
//!
//! # Assumptions to verify later
//!
//! - The top-level binary for print mode is `gh` with the `copilot
//!   suggest` subcommand. The standalone `copilot` binary exists on
//!   some distributions and may require a different invocation.
//! - Prompts are positional, not passed via a flag.
//!
//! Override via [`command`](CopilotAgent::command),
//! [`subcommand`](CopilotAgent::subcommand), and
//! [`args`](CopilotAgent::args).
//!
//! # Construction
//!
//! [`CopilotAgent`] exposes no project-shaped defaults. Every field on
//! [`CopilotSettings`] is required. Note that `subcommand` is a genuine
//! `Option`: `None` asks iter to apply its canonical one-shot subcommand
//! (`["copilot", "suggest"]`) which is agent-operational knowledge, not a
//! project-shaped decision; `Some(vec![])` means "invoke the binary with
//! no subcommand" (for standalone Copilot TUI builds).

use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize,
    inject_agent_otel_resource_attrs, inject_copilot_trace_parent_env, run_command,
};
use hook::HookBundle;

/// Canonical one-shot subcommand for `gh` — agent-operational knowledge
/// iter holds so users don't need to look up the Copilot CLI's shape.
const CANONICAL_SUBCOMMAND: &[&str] = &["copilot", "suggest"];

/// Fully-specified configuration for [`CopilotAgent`].
///
/// Every field is required. `subcommand` is a genuine `Option` (agent
/// operational default on `None`; explicit override on `Some`).
#[derive(Debug, Clone)]
pub struct CopilotSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Print vs. interactive mode.
    pub mode: AgentMode,
    /// Subcommand arguments inserted between the binary and the positional
    /// prompt. `None` → iter applies its canonical
    /// `["copilot", "suggest"]` (agent-operational knowledge). `Some(v)` →
    /// use `v` exactly; `Some(vec![])` means "no subcommand" (standalone
    /// `copilot` TUI).
    pub subcommand: Option<Vec<String>>,
    /// Additional arguments inserted between the subcommand and the prompt.
    /// Empty is allowed.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

/// GitHub Copilot CLI agent configuration.
#[derive(Debug, Clone)]
pub struct CopilotAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Subcommand arguments inserted between the binary and the positional
    /// prompt. `None` falls back to the canonical
    /// `["copilot", "suggest"]`; `Some(vec![])` invokes the binary with
    /// no subcommand at all.
    pub subcommand: Option<Vec<String>>,
    /// Additional arguments inserted between the subcommand and the prompt.
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl CopilotAgent {
    /// Build a fully-specified Copilot agent. Every knob must be
    /// supplied by the caller; iter provides no project-shaped defaults.
    #[must_use]
    pub fn new(settings: CopilotSettings) -> Self {
        let CopilotSettings {
            command,
            mode,
            subcommand,
            args,
            env,
        } = settings;
        Self {
            command,
            mode,
            subcommand,
            args,
            env,
        }
    }

    /// Shared argv builder. The same shape is used for both run modes —
    /// binary + subcommand + args + prompt — because Copilot's
    /// interactive and one-shot invocations both take a positional prompt
    /// as the final argument. Run-mode-specific plumbing (hook install,
    /// stdio inheritance) is layered on in the caller.
    fn build_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        match &self.subcommand {
            Some(sub) => {
                for arg in sub {
                    cmd.arg(arg);
                }
            }
            None => {
                for arg in CANONICAL_SUBCOMMAND {
                    cmd.arg(arg);
                }
            }
        }
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(prompt.as_str());
        cmd
    }
}

impl Agent for CopilotAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            service_name,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = self.build_command(workspace_path, prompt);
                apply_user_env(&mut command, &self.env);
                inject_agent_otel_resource_attrs(
                    &mut command,
                    signal_id,
                    signal_kind,
                    workspace_path,
                    "copilot",
                );
                inject_copilot_trace_parent_env(&mut command);
                run_command(command, PromptDelivery::Inline, cancel, stdio_sink).await
            }
            AgentMode::Interactive => {
                self.run_interactive(
                    workspace_path,
                    prompt,
                    cancel,
                    signal_id,
                    signal_kind,
                    &service_name,
                )
                .await
            }
        }
    }
}

impl CopilotAgent {
    /// Drive the Copilot CLI as a TUI session. Installs the project-local
    /// `agentStop` hook bundle before spawning and finalizes it after —
    /// even on error paths — so the user's original hook files are
    /// always restored.
    ///
    /// The run-then-finalize skeleton lives in
    /// [`drive_interactive_with_finalize`]; this method only handles the
    /// Copilot-specific bits: bundle install, command construction, and
    /// stdio inheritance wiring.
    #[allow(clippy::too_many_arguments)]
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
        service_name: &str,
    ) -> Result<AgentReport, AgentError> {
        let bundle = HookBundle::install(path, service_name).await?;

        let mut command = self.build_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "copilot");
        inject_copilot_trace_parent_env(&mut command);
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        drive_interactive_with_finalize(command, cancel, bundle.finalize()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>, mode: AgentMode) -> CopilotSettings {
        CopilotSettings {
            command: command.into(),
            mode,
            subcommand: None,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    #[tokio::test]
    async fn emits_copilot_suggest_subcommand() {
        let (_guard, bin) = fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done");
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-copilot");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("copilot"), "got {out:?}");
        assert!(out.contains("suggest"), "got {out:?}");
        assert!(out.contains("hello-copilot"), "got {out:?}");
    }

    #[tokio::test]
    async fn subcommand_can_be_overridden() {
        let (_guard, bin) = fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.subcommand = Some(Vec::new());
        let agent = CopilotAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(!out.contains("copilot"), "got {out:?}");
        assert!(out.contains(" x"), "got {out:?}");
    }

    #[tokio::test]
    async fn print_mode_injects_signal_resource_attributes() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$OTEL_RESOURCE_ATTRIBUTES\"");
        let tmp = TempDir::new().expect("tmp");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.subcommand = Some(Vec::new());
        let agent = CopilotAgent::new(s);
        let prompt = Prompt::from("x");

        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");

        assert!(out.contains("iter.signal.id="), "got {out:?}");
        assert!(out.contains("iter.signal.kind=work"), "got {out:?}");
        assert!(out.contains("iter.agent.driver=copilot"), "got {out:?}");
        assert!(
            out.contains(&format!(
                "iter.workspace.path={}",
                tmp.path().canonicalize().unwrap().display()
            )),
            "got {out:?}"
        );
    }

    /// Fake Copilot binary for interactive mode. Invokes the installed
    /// agentStop hook. The hook drains stdin and SIGKILLs `$PPID`.
    const FAKE_COPILOT_SCRIPT: &str = r#"
set -uo pipefail
HOOK="$PWD/.github/hooks/copilot-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_config() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_COPILOT_SCRIPT);

        let config_path = tmp.path().join(".github/hooks/copilot-loop.json");
        let script_path = tmp.path().join(".github/hooks/copilot-loop-hook.sh");
        fs::create_dir_all(config_path.parent().unwrap())
            .await
            .expect("mkdir .github/hooks");
        let user_config = json!({ "user_owned": true });
        fs::write(
            &config_path,
            serde_json::to_vec_pretty(&user_config).unwrap(),
        )
        .await
        .expect("write user config");
        let user_script = b"#!/usr/bin/env bash\necho user script\n";
        fs::write(&script_path, user_script)
            .await
            .expect("write user script");

        let mut s = settings(bin.to_string_lossy(), AgentMode::Interactive);
        s.subcommand = Some(Vec::new());
        let agent = CopilotAgent::new(s);

        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");

        assert!(
            report.exit_status == ExitStatus::Success
                || matches!(report.exit_status, ExitStatus::Signal(_)),
            "expected success or signal, got {:?}",
            report.exit_status,
        );

        let restored_config: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).await.expect("read")).expect("json");
        assert_eq!(restored_config, user_config);
        let restored_script = fs::read(&script_path).await.expect("read");
        assert_eq!(restored_script, user_script);

        assert!(
            !tmp.path().join(".github/hooks/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$COPILOT_TEST_ENV_VAR\"");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("COPILOT_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = CopilotAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value",);
    }
}
