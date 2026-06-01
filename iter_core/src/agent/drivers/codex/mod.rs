//! [`CodexAgent`] — `OpenAI` Codex CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   codex exec --json [extra-args...] <prompt>
//!   ```
//!
//!   The prompt is passed as the final positional argument and the
//!   child's stdin is closed immediately. `exec` is Codex's one-shot
//!   non-interactive mode; `--json` requests the machine-readable JSONL
//!   event stream the Command layer parses for the terminal turn status
//!   and session id. See [`command`] for the output contract.
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
//!   The hook's sole purpose is to terminate the TUI session after the
//!   agent finishes its task — it runs any pre-existing user Stop hooks,
//!   then sends SIGKILL to the Codex process. The hook is a direct
//!   descendant of
//!   [`agent-loop/codex-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to terminate the
//!   TUI session.
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

use crate::{Agent, AgentRun, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod command;
mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize,
    inject_agent_otel_resource_attrs, inject_trace_context_env, spawn_capture,
};
use command::{CodexCommand, CodexError};
use hook::HookBundle;

/// `-c` override that enables Codex's Stop hook protocol. Passed to the
/// interactive-mode command as a separate argument pair.
const CODEX_HOOKS_FEATURE_FLAG: &str = "features.codex_hooks=true";

impl From<CodexError> for AgentError {
    /// Adapter projection: collapse Codex's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. Only [`CodexError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; bad-args
    /// is a launch-class misconfiguration; the rest become the generic
    /// failure / signal variants.
    fn from(err: CodexError) -> Self {
        match err {
            CodexError::TokenLimit(detail) => Self::TokenLimit(detail),
            CodexError::Signal(sig) => Self::TerminatedBySignal(sig),
            CodexError::BadArgs => {
                Self::Launch("codex rejected the command-line arguments".to_owned())
            }
            CodexError::Reported {
                status,
                will_retry,
                exit_code,
            } => Self::Failed {
                code: exit_code,
                message: format!("codex reported turn status `{status}` (will_retry={will_retry})"),
            },
            CodexError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "codex produced no terminal turn status".to_owned(),
            },
        }
    }
}

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
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
            env,
        } = settings;
        Self {
            command,
            mode,
            args,
            env,
        }
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
    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
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
                let mut command = CodexCommand {
                    program: &self.command,
                    args: &self.args,
                    prompt: prompt.as_str(),
                }
                .build(workspace_path);
                apply_user_env(&mut command, &self.env);
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
                let output =
                    spawn_capture(command, PromptDelivery::Inline, cancel, stdio_sink).await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<CodexError>` above.
                let result = command::interpret(&output)?;
                Ok(AgentRun {
                    session_id: result.session_id,
                })
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
    #[allow(clippy::too_many_arguments)]
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
        service_name: &str,
    ) -> Result<AgentRun, AgentError> {
        let bundle = HookBundle::install(path, service_name).await?;

        let mut command = self.build_interactive_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "codex");
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        // Interactive mode has no machine-readable output: the only signal is
        // the child's exit. A clean exit is a run; anything else is a failure.
        let exit = drive_interactive_with_finalize(command, cancel, bundle.finalize()).await?;
        if let Some(err) = exit.into_failure() {
            return Err(err);
        }
        Ok(AgentRun::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{ctx, ctx_capturing, fake_binary_script};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>, mode: AgentMode) -> CodexSettings {
        CodexSettings {
            command: command.into(),
            mode,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `codex` print binary: echoes each argv arg to *stderr* (so a
    /// [`CaptureSink`] can observe them), then prints a valid terminal
    /// turn-status JSONL stream to stdout so the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s\n' '{"type":"session_configured","session_id":"sess-x"}'
printf '%s\n' '{"type":"agent_message","message":"ok"}'
printf '%s\n' '{"type":"task_complete","status":"completed"}'"#;

    #[tokio::test]
    async fn print_mode_passes_subcommand_and_inline_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-codex");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"exec"), "got {args:?}");
        assert!(args.contains(&"--json"), "got {args:?}");
        assert!(args.contains(&"hello-codex"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_extra_args_are_forwarded_before_prompt() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "o1".into()];
        let agent = CodexAgent::new(s);
        let prompt = Prompt::from("the-prompt");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let lines: Vec<&str> = echoed.lines().collect();
        // Argv order must be: exec --json --model o1 the-prompt
        let exec_pos = lines.iter().position(|l| *l == "exec").expect("exec");
        let model_pos = lines.iter().position(|l| *l == "--model").expect("--model");
        let prompt_pos = lines
            .iter()
            .position(|l| *l == "the-prompt")
            .expect("the-prompt");
        assert!(
            exec_pos < model_pos && model_pos < prompt_pos,
            "got {lines:?}"
        );
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$CODEX_TEST_ENV_VAR\" 1>&2\nprintf '%s\\n' '{\"type\":\"task_complete\",\"status\":\"completed\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("CODEX_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = CodexAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    #[tokio::test]
    async fn print_mode_failed_turn_maps_to_failed_error() {
        let script = r#"printf '%s\n' '{"type":"task_complete","status":"failed"}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("failed turn is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn print_mode_usage_limit_maps_to_token_limit() {
        let script = r#"printf '%s\n' '{"type":"error","message":"You'"'"'ve hit your usage limit."}'
printf '%s\n' '{"type":"task_complete","status":"failed"}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("usage limit is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_bad_args_exit_maps_to_launch() {
        let (_guard, bin) = fake_binary_script("printf 'bad\\n' 1>&2\nexit 2");
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("bad args is an error");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    /// Fake `codex` binary for interactive mode.
    ///
    /// Invokes the installed Stop hook. The hook drains stdin and
    /// SIGKILLs `$PPID` (this fake process), causing it to exit.
    const FAKE_CODEX_SCRIPT: &str = r#"
set -uo pipefail
HOOK="$PWD/.codex/hooks/codex-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_config() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_CODEX_SCRIPT);

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
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that the bundle was finalized.
        let _ignored = agent.run(ctx(tmp.path(), &prompt)).await;

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
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_finalizes_when_hook_never_fires() {
        // Fake codex that exits 0 without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 0");
        let tmp = TempDir::new().expect("tmp");
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        // A clean exit with no hook fired → an empty `AgentRun` and the hook
        // bundle is torn down regardless.
        let prompt = Prompt::from("probe");
        let run = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("interactive run ok");
        assert!(run.session_id.is_none());
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when the hook never fires",
        );
    }

    #[tokio::test]
    async fn interactive_mode_finalizes_even_when_child_fails() {
        // Fake codex that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let agent = CodexAgent::new(settings(bin.to_string_lossy(), AgentMode::Interactive));
        let prompt = Prompt::from("x");
        let result = agent.run(ctx(tmp.path(), &prompt)).await;

        // A non-zero exit is an `Err(Failed { code: Some(7) })`; the hook
        // bundle MUST still be cleaned up.
        let err = result.expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
        assert!(
            !tmp.path().join(".codex/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
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
