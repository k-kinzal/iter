//! [`CopilotDriver`] — GitHub Copilot CLI integration.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles:
//!
//!   ```text
//!   copilot -p <prompt> --allow-all-tools --output-format json [extra-args...]
//!   ```
//!
//!   `-p` is Copilot's one-shot print flag; `--output-format json` makes the
//!   terminal record machine-readable; `--allow-all-tools` stops the CLI
//!   blocking on per-tool confirmation. The argv shape lives at the Command
//!   level (`command.rs`); this driver only projects its result/error onto
//!   iter's domain. The child's stdin is closed immediately and stdout is
//!   captured for the Command to interpret.
//!
//! * [`AgentMode::Interactive`] — launches the configured Copilot CLI
//!   binary as a live TUI (`stdio: Inherit`) with a project-local
//!   `agentStop` hook installed under `${cwd}/.github/hooks/` by
//!   [`prepare`](crate::agent::AgentDriver::prepare) and restored by
//!   [`cleanup`](crate::agent::AgentDriver::cleanup). The hook bundle
//!   consists of **two** files (unlike the other three hook-based
//!   agents): `copilot-loop.json` (the hook config) and
//!   `copilot-loop-hook.sh` (the hook body). Both are backed up and
//!   restored.
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
//!   [`command`](CopilotDriver::command) + [`subcommand`](CopilotDriver::subcommand)
//!   must launch a live TUI that loads `.github/hooks/copilot-loop.json`
//!   on startup. The default (`gh copilot suggest`) is a one-shot print
//!   command and will *not* work in interactive mode; users must point
//!   `command` at the standalone `copilot` TUI binary and clear the
//!   subcommand first:
//!
//!   ```no_run
//!   # use iter_core::agent::{AgentMode, CopilotDriver};
//!   let driver = CopilotDriver {
//!       command: "copilot".into(),
//!       mode: AgentMode::Interactive,
//!       subcommand: Some(Vec::<String>::new()),
//!       args: Vec::new(),
//!       env: Vec::new(),
//!       hook_isolation_key: "default".into(),
//!   };
//!   ```
//!
//!   Interactive mode inherits stdin/stdout/stderr from the parent
//!   process so the TUI renders correctly when iter is invoked from a
//!   terminal. In non-tty environments (CI, detached runs) use
//!   [`AgentMode::Headless`] instead.
//!
//! # Assumptions to verify later
//!
//! - The top-level binary for print mode is `gh` with the `copilot
//!   suggest` subcommand. The standalone `copilot` binary exists on
//!   some distributions and may require a different invocation.
//! - Prompts are positional, not passed via a flag.
//!
//! Override via [`command`](CopilotDriver::command),
//! [`subcommand`](CopilotDriver::subcommand), and
//! [`args`](CopilotDriver::args).
//!
//! # Construction
//!
//! [`CopilotDriver`] exposes no project-shaped defaults. Every field is
//! required and the driver is constructed directly from its fields. Note that
//! `subcommand` is a genuine `Option`: `None` asks iter to apply its
//! canonical one-shot subcommand (`["copilot", "suggest"]`) which is
//! agent-operational knowledge, not a project-shaped decision; `Some(vec![])`
//! means "invoke the binary with no subcommand" (for standalone Copilot TUI
//! builds).

use std::path::Path;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;
use async_trait::async_trait;
use tokio::process::Command;

mod command;
mod hook;

use crate::agent::process::{
    RawOutput, apply_user_env, inject_agent_otel_resource_attrs, inject_copilot_trace_parent_env,
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

/// GitHub Copilot CLI driver configuration.
#[derive(Debug, Clone)]
pub struct CopilotDriver {
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
    /// Per-exploration hook isolation key: distinguishes one Runner's
    /// stop-hook installation from another's when both explore the same
    /// workspace path. `"default"` for standalone `iter run`.
    pub hook_isolation_key: String,
}

impl CopilotDriver {
    /// Interactive-mode argv builder: binary + subcommand + args + positional
    /// prompt. The interactive TUI takes the prompt as its final positional
    /// argument; print mode instead uses the [`CopilotCommand`] builder, which
    /// owns the `-p … --output-format json` shape.
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

#[async_trait]
impl AgentDriver for CopilotDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        _session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        match self.mode {
            AgentMode::Headless => {
                let mut process = CopilotCommand {
                    program: &self.command,
                    args: &self.args,
                    prompt: prompt.as_str(),
                }
                .build(path);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "copilot");
                inject_copilot_trace_parent_env(&mut process);
                Ok(AgentCommand {
                    // The prompt is embedded in argv via `-p`, so no stdin data.
                    process,
                    stdin: None,
                    io: StdioMode::Piped,
                })
            }
            AgentMode::Interactive => {
                let mut process = self.build_command(path, prompt);
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "copilot");
                inject_copilot_trace_parent_env(&mut process);
                Ok(AgentCommand {
                    process,
                    stdin: None,
                    io: StdioMode::Inherit,
                })
            }
        }
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        let raw = RawOutput::from(output);
        match self.mode {
            // Interactive mode has no machine-readable output: the only
            // signal is the child's exit. A clean exit is a run; anything
            // else is a failure.
            AgentMode::Interactive => match raw.exit.into_failure() {
                None => Ok(AgentRun::empty()),
                Some(err) => Err(err),
            },
            AgentMode::Headless => {
                // Adapter: project the Command's CLI-shaped result/error onto
                // iter's domain. `?` runs the `From<CopilotError>` above.
                let result = command::interpret(&raw)?;
                Ok(AgentRun {
                    session_id: result.session_id,
                })
            }
        }
    }

    async fn prepare(&self, path: &Path) -> Result<(), AgentError> {
        if matches!(self.mode, AgentMode::Interactive) {
            // The bundle handle is a pure path derivation; cleanup
            // reattaches to it, so the driver holds no state between the
            // two calls.
            drop(HookBundle::install(path, &self.hook_isolation_key).await?);
        }
        Ok(())
    }

    async fn cleanup(&self, path: &Path) -> Result<(), AgentError> {
        if matches!(self.mode, AgentMode::Interactive) {
            HookBundle::reattach(path).finalize().await?;
        }
        Ok(())
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Copilot
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RawExit;
    use crate::agent::testutil::{drive, drive_capturing, fake_binary_script};
    use serde_json::json;
    use std::ffi::OsStr;
    use tempfile::TempDir;
    use tokio::fs;

    fn copilot_driver(command: impl Into<String>, mode: AgentMode) -> CopilotDriver {
        CopilotDriver {
            command: command.into(),
            mode,
            subcommand: None,
            args: Vec::new(),
            env: Vec::new(),
            hook_isolation_key: "default".to_owned(),
        }
    }

    fn argv(command: &AgentCommand) -> Vec<String> {
        command
            .process
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn synth_output(exit: RawExit, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: exit.into_exit_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn headless_command_emits_print_json_and_allow_all_tools_flags() {
        let d = copilot_driver("copilot", AgentMode::Headless);
        let prompt = Prompt::from("hello-copilot");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"-p".to_owned()), "got {args:?}");
        assert!(args.contains(&"hello-copilot".to_owned()), "got {args:?}");
        assert!(
            args.contains(&"--allow-all-tools".to_owned()),
            "got {args:?}"
        );
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        assert_eq!(command.stdin, None, "copilot delivers the prompt as argv");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn headless_command_forwards_extra_args() {
        let mut d = copilot_driver("copilot", AgentMode::Headless);
        d.args = vec!["--model".into(), "gpt-5".into()];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"gpt-5".to_owned()), "got {args:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = copilot_driver("copilot", AgentMode::Headless);
        d.env = vec![("COPILOT_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == OsStr::new("COPILOT_TEST_ENV_VAR") && v == Some(OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    #[test]
    fn interactive_command_puts_prompt_last_and_inherits_stdio() {
        let mut d = copilot_driver("copilot", AgentMode::Interactive);
        // Clear the subcommand so the standalone TUI binary is invoked bare;
        // the canonical `copilot suggest` is a one-shot print command.
        d.subcommand = Some(Vec::new());
        d.args = vec!["--foo".into()];
        let prompt = Prompt::from("the-prompt");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert_eq!(
            args.last().map(String::as_str),
            Some("the-prompt"),
            "got {args:?}"
        );
        assert!(!args.contains(&"suggest".to_owned()), "got {args:?}");
        let foo_pos = args.iter().position(|a| a == "--foo").expect("--foo");
        let prompt_pos = args.iter().position(|a| a == "the-prompt").expect("prompt");
        assert!(foo_pos < prompt_pos, "got {args:?}");
        assert_eq!(command.stdin, None, "Inherit mode must not feed stdin");
        assert_eq!(command.io, StdioMode::Inherit);
    }

    #[test]
    fn interactive_command_defaults_to_canonical_subcommand() {
        let d = copilot_driver("gh", AgentMode::Interactive);
        let prompt = Prompt::from("go");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        // With `subcommand: None`, iter injects `copilot suggest`.
        assert_eq!(
            args.first().map(String::as_str),
            Some("copilot"),
            "got {args:?}"
        );
        assert!(args.contains(&"suggest".to_owned()), "got {args:?}");
        assert_eq!(args.last().map(String::as_str), Some("go"), "got {args:?}");
    }

    // ----- interpret(): inbound translation --------------------------------

    #[test]
    fn interpret_result_extracts_session_id() {
        let d = copilot_driver("copilot", AgentMode::Headless);
        let body = r#"{"type":"result","sessionId":"sess-x","exitCode":0}"#;
        let run = d
            .interpret(&synth_output(RawExit::Code(0), body, ""))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_quota_error_maps_to_token_limit() {
        let d = copilot_driver("copilot", AgentMode::Headless);
        let body = r#"{"type":"session.error","errorType":"quota_exceeded","statusCode":402}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body, ""))
            .expect_err("quota is an error");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    #[test]
    fn interpret_auth_error_maps_to_failed() {
        let d = copilot_driver("copilot", AgentMode::Headless);
        let body = r#"{"type":"session.error","errorType":"unauthorized","statusCode":401}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body, ""))
            .expect_err("auth is an error");
        assert!(
            matches!(
                err,
                AgentError::Failed {
                    code: Some(401),
                    ..
                }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_no_result_maps_to_failed() {
        let d = copilot_driver("copilot", AgentMode::Headless);
        let err = d
            .interpret(&synth_output(RawExit::Code(1), "garbage", ""))
            .expect_err("no result is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_interactive_judges_by_exit_only() {
        let d = copilot_driver("copilot", AgentMode::Interactive);
        assert!(d.interpret(&synth_output(RawExit::Code(0), "", "")).is_ok());
        let err = d
            .interpret(&synth_output(RawExit::Code(7), "", ""))
            .expect_err("non-zero exit");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "got {err:?}",
        );
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `copilot` print binary: echoes each argv arg to *stderr* (so a
    /// capture sink can observe them), then prints a valid terminal `result`
    /// JSON object to stdout so the driver parses an `Ok`.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"type":"result","sessionId":"sess-x","exitCode":0,"usage":{"premiumRequests":1}}'"#;

    #[tokio::test]
    async fn print_mode_passes_through_argv_and_parses_session() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = copilot_driver(bin.to_string_lossy(), AgentMode::Headless);
        let prompt = Prompt::from("hello-copilot");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"-p"), "got {args:?}");
        assert!(args.contains(&"hello-copilot"), "got {args:?}");
        assert!(args.contains(&"--allow-all-tools"), "got {args:?}");
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
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

        let mut d = copilot_driver(bin.to_string_lossy(), AgentMode::Interactive);
        d.subcommand = Some(Vec::new());

        let prompt = Prompt::from("go");
        // The fake either exits 0 (`Ok`) or is SIGKILLed by the hook
        // (`Err(TerminatedBySignal)`); the run result is racy and not what
        // this test asserts. What matters is that cleanup restored both files.
        let _ignored = drive(d, tmp.path(), &prompt).await;

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
    async fn interactive_mode_cleans_up_even_when_child_fails() {
        // Fake copilot that exits nonzero without touching the hook.
        let (_guard, bin) = fake_binary_script("exit 7");
        let tmp = TempDir::new().expect("tmp");
        let mut d = copilot_driver(bin.to_string_lossy(), AgentMode::Interactive);
        d.subcommand = Some(Vec::new());
        let prompt = Prompt::from("x");
        let result = drive(d, tmp.path(), &prompt).await;

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
