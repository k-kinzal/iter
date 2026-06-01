//! [`CopilotAgent`] — GitHub Copilot CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Print`] — the default. Spawns:
//!
//!   ```text
//!   copilot -p <prompt> --allow-all-tools --output-format json [extra-args...]
//!   ```
//!
//!   `-p` is Copilot's one-shot print flag; `--output-format json` makes the
//!   terminal record machine-readable; `--allow-all-tools` stops the CLI
//!   blocking on per-tool confirmation. The argv shape lives at the Command
//!   level (`command.rs`); this driver only projects its result/error onto
//!   iter's domain.
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

use crate::{Agent, AgentRun, AgentRunContext, Prompt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod command;
mod hook;

use crate::agent::AgentError;
use crate::agent::mode::AgentMode;
use crate::agent::process::{
    PromptDelivery, apply_user_env, drive_interactive_with_finalize,
    inject_agent_otel_resource_attrs, inject_copilot_trace_parent_env, spawn_capture,
};
use command::{CopilotCommand, CopilotError};
use hook::HookBundle;

impl From<CopilotError> for AgentError {
    /// Adapter projection: collapse Copilot's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. The router only branches on
    /// [`AgentError::TokenLimit`], so the three exhaustion classes
    /// (quota 402, rate 429, and any detected context/token limit) collapse
    /// there; auth, network, other reported errors, and the no-result case
    /// become [`AgentError::Failed`]; a terminating signal becomes
    /// [`AgentError::TerminatedBySignal`].
    fn from(err: CopilotError) -> Self {
        match err {
            CopilotError::QuotaExhausted { error_type, status } => Self::TokenLimit(format!(
                "copilot quota exhausted (status {status:?}): {error_type}"
            )),
            CopilotError::RateLimited { error_type, status } => Self::TokenLimit(format!(
                "copilot rate limited (status {status:?}): {error_type}"
            )),
            CopilotError::TokenLimit(detail) => Self::TokenLimit(detail),
            CopilotError::Auth { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot authentication failed (status {status:?}): {error_type}"),
            },
            CopilotError::Network { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot network error (status {status:?}): {error_type}"),
            },
            CopilotError::Reported { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot reported error `{error_type}` (status {status:?})"),
            },
            CopilotError::Signal(sig) => Self::TerminatedBySignal(sig),
            CopilotError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "copilot produced no terminal result".to_owned(),
            },
        }
    }
}

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

    /// Interactive-mode argv builder: binary + subcommand + args + positional
    /// prompt. The interactive TUI takes the prompt as its final positional
    /// argument; print mode instead uses the [`CopilotCommand`] builder, which
    /// owns the `-p … --output-format json` shape. Run-mode-specific plumbing
    /// (hook install, stdio inheritance) is layered on in the caller.
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
                let mut command = CopilotCommand {
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
                    "copilot",
                );
                inject_copilot_trace_parent_env(&mut command);
                // The prompt is embedded in argv via `-p`, so no stdin data.
                let output =
                    spawn_capture(command, PromptDelivery::Inline, cancel, stdio_sink).await?;
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<CopilotError>` above.
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
    ) -> Result<AgentRun, AgentError> {
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

    fn settings(command: impl Into<String>, mode: AgentMode) -> CopilotSettings {
        CopilotSettings {
            command: command.into(),
            mode,
            subcommand: None,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Fake `copilot` print binary: echoes each argv arg to *stderr* (so a
    /// [`CaptureSink`](crate::agent::testutil::CaptureSink) can observe them),
    /// then prints a valid terminal `result` JSON object to stdout so the
    /// Command parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"type":"result","sessionId":"sess-x","exitCode":0,"usage":{"premiumRequests":1}}'"#;

    #[tokio::test]
    async fn print_mode_emits_print_json_and_allow_all_tools_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("hello-copilot");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"-p"), "got {args:?}");
        assert!(args.contains(&"hello-copilot"), "got {args:?}");
        assert!(args.contains(&"--allow-all-tools"), "got {args:?}");
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_extra_args_are_forwarded() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.args = vec!["--model".into(), "gpt-5".into()];
        let agent = CopilotAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"gpt-5"), "got {args:?}");
    }

    #[tokio::test]
    async fn print_mode_quota_error_maps_to_token_limit() {
        let script = r#"printf '%s' '{"type":"session.error","errorType":"quota_exceeded","statusCode":402}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("quota is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn print_mode_auth_error_maps_to_failed() {
        let script = r#"printf '%s' '{"type":"session.error","errorType":"unauthorized","statusCode":401}'
exit 1"#;
        let (_guard, bin) = fake_binary_script(script);
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("auth is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(401), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn print_mode_no_result_maps_to_failed() {
        let (_guard, bin) = fake_binary_script("printf 'garbage'\nexit 1");
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");
        let (ctx, _sink) = ctx_capturing(Path::new("."), &prompt);
        let err = agent.run(ctx).await.expect_err("no result is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[tokio::test]
    async fn print_mode_injects_signal_resource_attributes() {
        let script = "printf '%s\\n' \"$OTEL_RESOURCE_ATTRIBUTES\" 1>&2\nprintf '%s' '{\"type\":\"result\",\"sessionId\":\"s\",\"exitCode\":0}'";
        let (_guard, bin) = fake_binary_script(script);
        let tmp = TempDir::new().expect("tmp");
        let agent = CopilotAgent::new(settings(bin.to_string_lossy(), AgentMode::Print));
        let prompt = Prompt::from("x");

        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");
        let out = sink.stderr().await;

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

    #[tokio::test]
    async fn print_mode_env_is_forwarded_to_child() {
        let script = "printf 'ENV=%s\\n' \"$COPILOT_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"type\":\"result\",\"sessionId\":\"s\",\"exitCode\":0}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = settings(bin.to_string_lossy(), AgentMode::Print);
        s.env = vec![("COPILOT_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = CopilotAgent::new(s);
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
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
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that the bundle was finalized.
        let _ignored = agent.run(ctx(tmp.path(), &prompt)).await;

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
    async fn interactive_mode_finalizes_even_when_child_fails() {
        // Fake copilot that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let mut s = settings(bin.to_string_lossy(), AgentMode::Interactive);
        s.subcommand = Some(Vec::new());
        let agent = CopilotAgent::new(s);
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
            !tmp.path().join(".github/hooks/.iter-bundle").exists(),
            ".iter-bundle must be cleaned up even when child fails",
        );
    }
}
