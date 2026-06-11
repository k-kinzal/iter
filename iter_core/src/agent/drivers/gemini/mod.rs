//! [`GeminiAgent`] — Google Gemini CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   gemini -p <prompt> -o json [extra-args...]
//!   ```
//!
//!   The prompt is delivered as the value of `-p`, matching the common
//!   `gemini -p 'explain foo'` invocation pattern, and `-o json` requests
//!   the machine-readable terminal record the [`command`] layer parses. The
//!   child's stdin is closed immediately and stdout is captured for the
//!   Command to interpret into an [`AgentRun`] or [`AgentError`].
//!
//! * [`AgentMode::Interactive`] — launches `gemini` as a live TUI with a
//!   project-local `AfterAgent` hook installed under `${cwd}/.gemini/`.
//!   The hook's sole purpose is to terminate the TUI session after the
//!   agent finishes its task — it runs any pre-existing user hooks,
//!   then sends SIGKILL to the Gemini CLI process.
//!
//!   The hook is a direct descendant of
//!   [`agent-loop/gemini-loop`](https://github.com/k-kinzal/agent-loop)'s
//!   wrapper but simplified: iter's [`Runner`](crate::Runner) handles
//!   signal-level iteration, so the hook only needs to terminate the
//!   TUI session.
//!
//!   **Project-local, not global.** Every path the hook touches lives
//!   under `${cwd}/.gemini/`. iter never writes to `~/.gemini/` because
//!   doing so would silently affect every other Gemini CLI session on
//!   the machine. See the `hook` submodule for the
//!   filesystem layout.
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process so `gemini`'s TUI renders correctly when iter is invoked
//!   from a terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Print`] instead.
//!
//! # Construction
//!
//! [`GeminiAgent`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use std::path::Path;
use std::process::Stdio;

use crate::{Agent, AgentRun, AgentRunContext, Prompt};
use async_trait::async_trait;
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
use command::{GeminiCommand, GeminiError};
use hook::HookBundle;

impl From<GeminiError> for AgentError {
    /// Adapter projection: collapse the Gemini CLI's CLI-shaped error
    /// hierarchy onto iter's minimal domain error.
    ///
    /// * Context/token-limit → [`AgentError::TokenLimit`] (router-relevant).
    /// * Fatal startup exit codes (auth / input / sandbox / config /
    ///   turn-limit) → [`AgentError::Launch`] — the agent never ran a turn.
    /// * Signal termination → [`AgentError::TerminatedBySignal`].
    /// * Everything else → [`AgentError::Failed`].
    fn from(err: GeminiError) -> Self {
        match err {
            GeminiError::TokenLimit(detail) => Self::TokenLimit(detail),
            GeminiError::Startup { exit_code, message } => Self::Launch(match message {
                Some(msg) => format!("gemini startup failure (exit code {exit_code}): {msg}"),
                None => format!("gemini startup failure (exit code {exit_code})"),
            }),
            GeminiError::Signal(sig) => Self::TerminatedBySignal(sig),
            GeminiError::Reported {
                error_type,
                message,
                code,
            } => Self::Failed {
                code,
                message: match (error_type, message) {
                    (Some(t), Some(m)) => format!("gemini reported error `{t}`: {m}"),
                    (Some(t), None) => format!("gemini reported error `{t}`"),
                    (None, Some(m)) => format!("gemini reported error: {m}"),
                    (None, None) => "gemini reported an error result".to_owned(),
                },
            },
            GeminiError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "gemini produced no result".to_owned(),
            },
        }
    }
}

/// Gemini CLI agent configuration.
#[derive(Debug, Clone)]
pub struct GeminiAgent {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Additional arguments appended after the built-in flags (the
    /// `-p <prompt> -o json` triple in print mode, or the prompt positional
    /// in interactive mode).
    pub args: Vec<String>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

impl GeminiAgent {
    /// Build the interactive-mode command. Passes the prompt as the
    /// first positional argument so `gemini` seeds its initial user
    /// turn with it before dropping into the TUI; extras come after so
    /// users can still inject their own flags.
    fn build_interactive_command(&self, path: &Path, prompt: &Prompt) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        cmd.arg(prompt.as_str());
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

#[async_trait]
impl Agent for GeminiAgent {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            signal_id,
            signal_kind,
            hook_isolation_key,
            ..
        } = ctx;
        match self.mode {
            AgentMode::Print => {
                let mut command = GeminiCommand {
                    program: &self.command,
                    prompt: prompt.as_str(),
                    args: &self.args,
                }
                .build(workspace_path);
                apply_user_env(&mut command, &self.env);
                inject_agent_otel_resource_attrs(
                    &mut command,
                    signal_id,
                    signal_kind,
                    workspace_path,
                    "gemini",
                );
                inject_trace_context_env(&mut command);
                let output = spawn_capture(
                    command,
                    // The prompt is already on the argv (`-p`), so no stdin
                    // payload is sent; stdin is closed immediately.
                    PromptDelivery::Inline,
                    cancel,
                    stdio_sink,
                )
                .await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<GeminiError>` above.
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
                    &hook_isolation_key,
                )
                .await
            }
        }
    }
}

impl GeminiAgent {
    /// Drive `gemini` as a TUI session. Installs the workspace-local
    /// `AfterAgent` hook bundle before spawning and finalizes it after
    /// — even on error paths — so the user's original `settings.json`
    /// is always restored.
    ///
    /// The run-then-finalize skeleton lives in
    /// [`drive_interactive_with_finalize`]; this method only handles the
    /// Gemini-specific bits: bundle install, command construction, and
    /// stdio inheritance wiring.
    async fn run_interactive(
        &self,
        path: &Path,
        prompt: &Prompt,
        cancel: CancellationToken,
        signal_id: crate::signal::SignalId,
        signal_kind: crate::signal::SignalKind,
        hook_isolation_key: &str,
    ) -> Result<AgentRun, AgentError> {
        let bundle = HookBundle::install(path, hook_isolation_key).await?;

        let mut command = self.build_interactive_command(path, prompt);
        apply_user_env(&mut command, &self.env);
        inject_agent_otel_resource_attrs(&mut command, signal_id, signal_kind, path, "gemini");
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

    fn gemini_agent(command: impl Into<String>, mode: AgentMode) -> GeminiAgent {
        GeminiAgent {
            command: command.into(),
            mode,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `gemini` print binary: echoes each argv arg to *stderr* (so a
    /// [`CaptureSink`] can observe them), then prints a valid terminal JSON
    /// object to stdout so the Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"response":"ok","stats":{"tokens":{"input":1,"output":2,"total":3}}}'"#;

    #[tokio::test]
    async fn emits_dash_p_prompt_and_json_format() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("hello-gemini");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id, None);
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"-p"), "got {args:?}");
        assert!(args.contains(&"hello-gemini"), "got {args:?}");
        assert!(args.contains(&"-o"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
        let dash_pos = args.iter().position(|a| *a == "-p").expect("-p");
        let prompt_pos = args
            .iter()
            .position(|a| *a == "hello-gemini")
            .expect("prompt");
        assert!(dash_pos < prompt_pos, "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_extra_args_are_forwarded_after_managed_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "gemini-pro".into()];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"gemini-pro"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_parses_session_id_when_present() {
        let script = r#"printf '%s' '{"response":"ok","session_id":"conv-9"}'"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("conv-9"));
    }

    #[tokio::test]
    async fn print_mode_error_field_maps_to_failed() {
        let script = r#"printf '%s' '{"error":{"type":"ApiError","message":"boom","code":7}}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("err");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn print_mode_startup_exit_code_maps_to_launch() {
        let (_guard, bin) = fake_binary_script("exit 41");
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("err");
        assert!(matches!(err, AgentError::Launch(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_context_error_maps_to_token_limit() {
        let script = r#"printf '%s' '{"error":{"type":"ContextLengthExceeded","message":"too big"}}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("err");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let script =
            "printf 'ENV=%s\\n' \"$GEMINI_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"response\":\"ok\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = gemini_agent(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("GEMINI_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    /// Fake `gemini` binary for interactive mode. Invokes the installed
    /// `AfterAgent` hook. The hook drains stdin and SIGKILLs `$PPID`.
    const FAKE_GEMINI_SCRIPT: &str = r#"
set -euo pipefail
HOOK="$PWD/.gemini/hooks/gemini-loop-hook.sh"
printf '{}' | "$HOOK" > /dev/null 2>&1 || true
exit 0
"#;

    #[tokio::test]
    async fn interactive_mode_installs_hook_and_restores_settings() {
        let tmp = TempDir::new().expect("tmp");
        let (_guard, bin) = fake_binary_script(FAKE_GEMINI_SCRIPT);

        let settings_path = tmp.path().join(".gemini/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap())
            .await
            .expect("mkdir .gemini");
        let user_settings = json!({ "user_owned": true });
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&user_settings).unwrap(),
        )
        .await
        .expect("write settings");

        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Interactive);

        let prompt = Prompt::from("go");
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that the bundle was finalized.
        let _ignored = agent.run(ctx(tmp.path(), &prompt)).await;

        let restored: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).await.expect("read")).expect("json");
        assert_eq!(
            restored, user_settings,
            "user settings.json must be restored after interactive run",
        );
        assert!(
            !tmp.path().join(".gemini/hooks").exists(),
            "hooks directory must be cleaned up",
        );
        assert!(
            !tmp.path().join(".gemini/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up",
        );
    }

    #[tokio::test]
    async fn interactive_mode_finalizes_even_when_child_fails() {
        // Fake gemini that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let agent = gemini_agent(bin.to_string_lossy(), AgentMode::Interactive);
        let prompt = Prompt::from("x");
        let result = agent.run(ctx(tmp.path(), &prompt)).await;

        // A non-zero exit is an `Err(Failed { code: Some(7) })` — the agent
        // ran no clean turn. The hook bundle MUST still be cleaned up.
        let err = result.expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
        assert!(
            !tmp.path().join(".gemini/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }
}
