//! [`CodexAgent`] — `OpenAI` Codex CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   codex exec [extra-args...] <prompt>
//!   ```
//!
//!   The prompt is passed as the final positional argument and the
//!   child's stdin is closed immediately. `exec` is Codex's one-shot
//!   non-interactive mode — a clean, observable shape for the
//!   `AgentFinished` event payload.
//!
//! * [`AgentMode::Interactive`] — launches `codex` as a live TUI with a
//!   project-local Stop hook installed under `${cwd}/.codex/`. Codex
//!   ships Claude-Code-style Stop hooks behind a CLI feature flag, so
//!   interactive mode invokes the binary as:
//!
//!   ```text
//!   codex -c "features.codex_hooks=true" [extra-args...] <prompt>
//!   ```
//!
//!   The hook is a direct descendant of
//!   [`agent-loop/codex-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to capture the final
//!   assistant message and let `codex` exit cleanly. On return the agent
//!   parses the captured transcript in Rust and populates
//!   [`AgentReport::last_output`](crate::AgentReport) with the last
//!   assistant text message, matching the shape of print mode so event
//!   sinks see the same output surface across modes.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.codex/`. iter never writes to `~/.codex/` because
//!   doing so would silently affect every other Codex session on the
//!   machine. See the `hook` submodule for the filesystem
//!   layout.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process so `codex`'s TUI renders correctly when iter is invoked
//!   from a terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Print`] instead.
//!
//! # Assumptions to verify later
//!
//! - The subcommand for print mode is `exec`. Some Codex builds use
//!   `run` or a bare prompt.
//! - `codex` accepts `-c "features.codex_hooks=true"` to enable the Stop
//!   hook protocol in interactive mode.
//! - The prompt is a positional argument, not a `--prompt=...` flag.
//!
//! Override [`args`](CodexAgent::args) to swap the subcommand or inject
//! flags without recompiling.
//!
//! # Construction
//!
//! [`CodexAgent`] exposes no defaults. Every field on [`CodexSettings`]
//! is required because the value is a project-shaped decision iter
//! cannot honestly pick on the operator's behalf.

use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, drive_interactive_with_finalize, inject_agent_otel_resource_attrs,
    inject_trace_context_env, run_command,
};
use hook::HookBundle;

/// `-c` override that enables Codex's Stop hook protocol. Passed to the
/// interactive-mode command as a separate argument pair.
const CODEX_HOOKS_FEATURE_FLAG: &str = "features.codex_hooks=true";

/// Fully-specified configuration for [`CodexAgent`].
///
/// Every field is required; there is no `Default` impl because every
/// value is a project-shaped decision the Iterfile must spell out
/// explicitly.
#[derive(Debug, Clone)]
pub struct CodexSettings {
    /// Binary name or absolute path.
    pub command: String,
    /// Print vs. interactive mode.
    pub mode: AgentMode,
    /// Additional arguments inserted between the `exec` subcommand (or,
    /// in interactive mode, between the `-c` feature flag pair) and the
    /// positional prompt. Empty is allowed.
    pub args: Vec<String>,
}

/// `OpenAI` Codex agent configuration.
#[derive(Debug, Clone)]
pub struct CodexAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments inserted between the `exec` subcommand (or,
    /// in interactive mode, between the `-c` feature flag pair) and the
    /// positional prompt.
    pub args: Vec<String>,
}

impl CodexAgent {
    /// Build a fully-specified Codex agent. Every knob must be decided
    /// by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: CodexSettings) -> Self {
        let CodexSettings {
            command,
            mode,
            args,
        } = settings;
        Self {
            command,
            mode,
            args,
        }
    }

    fn build_print_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("exec");
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(prompt.as_str());
        cmd
    }

    /// Build the interactive-mode command. Passes the Codex hooks
    /// feature flag via `-c` so the installed Stop hook actually fires,
    /// then any user-supplied extras, then the prompt as the final
    /// positional argument so `codex` seeds its initial user turn before
    /// dropping into the TUI.
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg("-c").arg(CODEX_HOOKS_FEATURE_FLAG);
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd.arg(prompt.as_str());
        cmd
    }
}

impl Agent for CodexAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = self.build_print_command(workspace_path, prompt);
                inject_agent_otel_resource_attrs(
                    &mut command,
                    signal_id,
                    signal_kind,
                    workspace_path,
                    "codex",
                );
                // `codex exec` imports W3C trace context from TRACEPARENT /
                // TRACESTATE. The TUI path is not treated as verified here.
                inject_trace_context_env(&mut command);
                run_command(command, PromptDelivery::Inline, cancel, stdio_sink).await
            }
            AgentMode::Interactive => {
                self.run_interactive(workspace_path, prompt, cancel, signal_id, signal_kind)
                    .await
            }
        }
    }
}

impl CodexAgent {
    /// Drive `codex` as a TUI session. Installs the project-local Stop
    /// hook bundle before spawning and finalizes it after — even on
    /// error paths — so the user's original `hooks.json` is always
    /// restored.
    ///
    /// The run-then-finalize skeleton lives in
    /// [`drive_interactive_with_finalize`]; this method only handles the
    /// Codex-specific bits: bundle install, command construction, and
    /// stdio inheritance wiring.
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
    ) -> Result<AgentReport, AgentError> {
        let bundle = HookBundle::install(path).await?;

        let mut command = self.build_interactive_command(path, prompt);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "codex");
        let (env_key, state_file) = bundle.env_var();
        command.env(env_key, state_file);
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

    fn settings(command: impl Into<String>, mode: AgentMode) -> CodexSettings {
        CodexSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
        }
    }

    #[tokio::test]
    async fn passes_subcommand_and_inline_prompt() {
        let (_guard, bin) = fake_binary_script(
            "printf 'args:'; for a in \"$@\"; do printf ' %s' \"$a\"; done; printf '\\n'",
        );
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-codex");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        assert!(out.contains("args: exec"), "got {out:?}");
        assert!(out.contains("hello-codex"), "got {out:?}");
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_before_prompt() {
        let (_guard, bin) = fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "o1".into()];
        let agent = CodexAgent::new(s);
        let prompt = Prompt::from("the-prompt");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        // Argv order must be: exec --model o1 the-prompt
        let exec_pos = out.find("exec").expect("exec");
        let model_pos = out.find("--model").expect("--model");
        let prompt_pos = out.find("the-prompt").expect("the-prompt");
        assert!(
            exec_pos < model_pos && model_pos < prompt_pos,
            "got {out:?}"
        );
    }

    /// Fake `codex` binary for interactive mode.
    ///
    /// Writes a single-message Claude-Code-shaped transcript to a
    /// well-known path in its cwd, then invokes the installed Stop hook
    /// with a payload that references that transcript. Drives the real
    /// [`HookBundle::finalize`] path end-to-end without needing a tty or
    /// the actual `codex` binary.
    ///
    /// The hook script sends `SIGTERM` to its parent as a
    /// belt-and-suspenders against Codex builds that ignore the JSON
    /// continuation response. The real `codex` binary catches that
    /// signal and exits cleanly; our fake binary does the same via a
    /// `trap … TERM` so the test asserts a clean exit status.
    const FAKE_CODEX_SCRIPT: &str = r#"
trap 'exit 0' TERM
set -uo pipefail
TRANSCRIPT="$PWD/.codex/iter-test-transcript.jsonl"
mkdir -p "$(dirname "$TRANSCRIPT")"
cat > "$TRANSCRIPT" <<'EOF'
{"type":"assistant","message":{"content":[{"type":"text","text":"CODEX-INTERACTIVE-DONE"}]}}
EOF
PAYLOAD=$(printf '{"session_id":"test","transcript_path":"%s","stop_hook_active":false}' "$TRANSCRIPT")
HOOK="$PWD/.codex/hooks/codex-loop-hook.sh"
printf '%s' "$PAYLOAD" | "$HOOK" > /dev/null || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_captures_last_message() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_CODEX_SCRIPT);

        // Drop a user-authored hooks.json so we can assert it gets
        // restored intact at the end.
        let hooks_path = tmp.path().join(".codex/hooks.json");
        fs::create_dir_all(hooks_path.parent().unwrap())
            .await
            .expect("mkdir .codex");
        let user_hooks = json!({ "user_owned": true });
        fs::write(&hooks_path, serde_json::to_vec_pretty(&user_hooks).unwrap())
            .await
            .expect("write hooks.json");

        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("go");
        let report = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");

        assert_eq!(report.exit_status, ExitStatus::Success);
        assert_eq!(
            report.last_output.as_deref(),
            Some("CODEX-INTERACTIVE-DONE"),
            "hook must capture the final assistant text message",
        );
        assert_eq!(report.turn_count, Some(1));

        // hooks.json must have been restored.
        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&hooks_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_hooks,
            "user hooks.json must be restored after interactive run",
        );
        assert!(
            !tmp.path().join(".codex/hooks/codex-loop-hook.sh").exists(),
            "hook script must be cleaned up",
        );
        assert!(
            !tmp.path().join(".codex/iter-state.json").exists(),
            "state file must be cleaned up",
        );
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_builds_with_feature_flag_pair() {
        // Fake codex that echoes argv so we can assert the flag pair.
        let (_guard, bin) =
            fake_binary_script("for a in \"$@\"; do printf ' %s' \"$a\"; done; exit 0");
        let tmp = TempDir::new().expect("tmp");
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        // Interactive mode uses inherit() for stdio so argv is not
        // captured by run_command's tail; instead, assert the hook
        // bundle was torn down cleanly on a nonzero exit.
        //
        // The interactive shape is covered at the unit level by
        // `build_interactive_command` below via the full happy-path test;
        // here we only verify that a child that never touches the hook
        // still finalizes cleanly (exit 0 with no hook fired → empty
        // capture, bundle cleaned).
        let prompt = Prompt::from("probe");
        let report = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        assert!(report.last_output.is_none());
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when the hook never fires",
        );
    }

    #[test]
    fn build_interactive_command_contains_feature_flag_pair_and_prompt_last() {
        // Unit-level assertion over the argv shape (run_command is not
        // involved here). We only inspect the Command's debug output
        // which includes argv in stable order.
        let mut s = settings("codex", AgentMode::Interactive);
        s.args = vec!["--model".into(), "gpt-5".into()];
        let agent = CodexAgent::new(s);
        let cmd = agent.build_interactive_command(Path::new("."), &Prompt::from("the-prompt"));
        let dbg = format!("{cmd:?}");
        // Ordering: `-c` before the flag, feature flag before extras,
        // extras before the prompt.
        let c_pos = dbg.find("\"-c\"").expect("-c present");
        let feat_pos = dbg.find(CODEX_HOOKS_FEATURE_FLAG).expect("feature flag");
        let model_pos = dbg.find("\"--model\"").expect("--model");
        let prompt_pos = dbg.find("\"the-prompt\"").expect("prompt");
        assert!(c_pos < feat_pos);
        assert!(feat_pos < model_pos);
        assert!(model_pos < prompt_pos);
    }
}
