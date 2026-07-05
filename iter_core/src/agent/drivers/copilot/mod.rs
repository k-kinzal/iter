//! [`CopilotDriver`] — GitHub Copilot CLI integration.
//!
//! The argv shape and JSONL output parsing live in the standalone
//! [`copilot_cli`] crate; this driver projects the crate's CLI-shaped result
//! and error onto iter's domain and owns the two run modes.
//!
//! Two run modes are supported:
//!
//! * [`AgentMode::Headless`] — the default. Assembles, via
//!   [`copilot_cli::RunCommand`]:
//!
//!   ```text
//!   copilot --prompt <prompt> --allow-all-tools --output-format json [extra-args...]
//!   ```
//!
//!   `--prompt` (`-p`) is Copilot's one-shot flag; `--output-format json`
//!   makes the terminal record machine-readable; `--allow-all-tools` stops the
//!   CLI blocking on per-tool confirmation (iter's sandbox is the real
//!   boundary). The child's stdin is closed immediately and stdout is captured
//!   for the driver to interpret against the crate's [`copilot_cli::RunOutput`].
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
//!   on startup. Standalone `copilot` 1.0.49 has **no `suggest` subcommand**
//!   (that was a `gh copilot` relic): the root `copilot` invocation *is* the
//!   interactive session, so the canonical subcommand is empty and
//!   `subcommand: None` injects nothing.
//!
//!   ```no_run
//!   # use iter_core::agent::{AgentMode, CopilotDriver};
//!   let driver = CopilotDriver {
//!       command: "copilot".into(),
//!       mode: AgentMode::Interactive,
//!       subcommand: None,
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
//! # Construction
//!
//! [`CopilotDriver`] exposes no project-shaped defaults. Every field is
//! required and the driver is constructed directly from its fields. `subcommand`
//! is a genuine `Option`: `None` asks iter to apply its canonical subcommand,
//! which is now **empty** (no injected verb); `Some(vec![...])` invokes the
//! binary with an explicit subcommand for non-default Copilot distributions.

use std::path::Path;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::{AgentError, AgentKind, AgentMode, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;
use async_trait::async_trait;
use copilot_cli::{Copilot, RunCommand, RunOptions, RunOutput, SessionError};
use thiserror::Error;

mod hook;

use crate::agent::process::{
    RawExit, RawOutput, apply_user_env, detect_token_limit, inject_agent_otel_resource_attrs,
};
use hook::HookBundle;

/// Inject the current trace context in the form GitHub Copilot CLI consumes.
///
/// The standalone Copilot CLI 1.0.43 does not read `TRACEPARENT` as an
/// incoming `OTel` carrier. Its SDK reads `COPILOT_TRACE_PARENT` and forwards it
/// to Copilot API calls as `X-Copilot-Traceparent`, so keep this path explicit
/// instead of reusing the generic environment-carrier helper.
fn inject_copilot_trace_parent_env(command: &mut tokio::process::Command) -> bool {
    let Some(traceparent) = iter_tracing::current_traceparent() else {
        return false;
    };
    command.env("COPILOT_TRACE_PARENT", traceparent);
    true
}

/// CLI-shaped error hierarchy for the Copilot CLI's headless output.
///
/// Carries the failure class plus the HTTP-ish `statusCode` the CLI surfaced.
/// Spawn-level concerns (cancellation, launch failure) live elsewhere; this is
/// only the output-interpretation error. Mirrors `claude_code`'s
/// `ClaudeCodeOutputError`: a driver-local CLI error the Adapter projects onto
/// iter's domain.
#[derive(Debug, Error)]
pub(crate) enum CopilotOutputError {
    /// Quota exhausted (`session.error` with `statusCode` 402). Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`].
    #[error("copilot quota exhausted (status {status:?}): {error_type}")]
    QuotaExhausted {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (expected 402).
        status: Option<u16>,
    },
    /// Rate limited (`session.error` with `statusCode` 429). Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`] (rate exhaustion is
    /// the closest domain class).
    #[error("copilot rate limited (status {status:?}): {error_type}")]
    RateLimited {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (expected 429).
        status: Option<u16>,
    },
    /// Context-window / token-limit detected in the output text. Router-relevant:
    /// the Adapter maps this to [`AgentError::TokenLimit`].
    #[error("copilot hit the context/token limit: {0}")]
    TokenLimit(String),
    /// Authentication / authorization failure (`statusCode` 401/403).
    #[error("copilot authentication failed (status {status:?}): {error_type}")]
    Auth {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (401 or 403).
        status: Option<u16>,
    },
    /// Network / server-side failure (`statusCode` 5xx).
    #[error("copilot network error (status {status:?}): {error_type}")]
    Network {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record (5xx).
        status: Option<u16>,
    },
    /// Any other reported `session.error` that does not fall into the classes
    /// above.
    #[error("copilot reported an error (`{error_type}`, status {status:?})")]
    Reported {
        /// `errorType` from the `session.error` record.
        error_type: String,
        /// `statusCode` from the `session.error` record, when present.
        status: Option<u16>,
    },
    /// The process was terminated by a signal before producing a result.
    #[error("copilot was terminated by signal {0}")]
    Signal(i32),
    /// The process exited without a parseable terminal record or
    /// `session.error` (e.g. exit 1 with no JSON).
    #[error("copilot produced no terminal result (exit code {exit_code:?})")]
    NoResult {
        /// Process exit code, when one was produced.
        exit_code: Option<i32>,
    },
}

impl From<CopilotOutputError> for AgentError {
    /// Adapter projection: collapse Copilot's CLI-shaped error hierarchy onto
    /// iter's minimal domain error. The router only branches on
    /// [`AgentError::TokenLimit`], so the three exhaustion classes
    /// (quota 402, rate 429, and any detected context/token limit) collapse
    /// there; auth, network, other reported errors, and the no-result case
    /// become [`AgentError::Failed`]; a terminating signal becomes
    /// [`AgentError::TerminatedBySignal`].
    fn from(err: CopilotOutputError) -> Self {
        match err {
            CopilotOutputError::QuotaExhausted { error_type, status } => Self::TokenLimit(format!(
                "copilot quota exhausted (status {status:?}): {error_type}"
            )),
            CopilotOutputError::RateLimited { error_type, status } => Self::TokenLimit(format!(
                "copilot rate limited (status {status:?}): {error_type}"
            )),
            CopilotOutputError::TokenLimit(detail) => Self::TokenLimit(detail),
            CopilotOutputError::Auth { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot authentication failed (status {status:?}): {error_type}"),
            },
            CopilotOutputError::Network { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot network error (status {status:?}): {error_type}"),
            },
            CopilotOutputError::Reported { error_type, status } => Self::Failed {
                code: status.map(i32::from),
                message: format!("copilot reported error `{error_type}` (status {status:?})"),
            },
            CopilotOutputError::Signal(sig) => Self::TerminatedBySignal(sig),
            CopilotOutputError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "copilot produced no terminal result".to_owned(),
            },
        }
    }
}

/// Interpret Copilot's headless output into a run outcome or a CLI-shaped error.
///
/// The crate's [`RunOutput`] surfaces both terminal records; the domain
/// judgement stays here: a `session.error` record — when present — is
/// authoritative (its presence *is* the failure signal), and the token-limit
/// refinement and signal handling live at this layer because they draw on the
/// process exit and stderr, not just the JSONL stream.
fn interpret_output(raw: &RawOutput<'_>) -> Result<AgentRun, CopilotOutputError> {
    let stdout = raw.stdout_str();
    let parsed = RunOutput::parse(&stdout);

    let exit_code = match raw.exit {
        RawExit::Code(c) => Some(c),
        RawExit::Signal(_) | RawExit::Unknown => None,
    };

    // Presence of `session.error` is the failure signal, authoritative over any
    // terminal `result` that may also appear.
    if let Some(error) = parsed.session_error() {
        return Err(classify_session_error(&error, &stdout));
    }

    let Some(result) = parsed.result() else {
        // Never produced a terminal record → never ran a turn.
        if let RawExit::Signal(sig) = raw.exit {
            return Err(CopilotOutputError::Signal(sig));
        }
        if let Some(detail) = detect_token_limit(&stdout) {
            return Err(CopilotOutputError::TokenLimit(detail));
        }
        let stderr = raw.stderr_str();
        if let Some(detail) = detect_token_limit(&stderr) {
            return Err(CopilotOutputError::TokenLimit(detail));
        }
        return Err(CopilotOutputError::NoResult { exit_code });
    };

    Ok(AgentRun {
        session_id: result.session_id,
    })
}

/// Map a crate-parsed `session.error` record onto the matching
/// [`CopilotOutputError`] class.
fn classify_session_error(record: &SessionError, stdout: &str) -> CopilotOutputError {
    let error_type = record.error_type.clone().unwrap_or_default();
    let status = record.status_code;
    match status {
        Some(402) => CopilotOutputError::QuotaExhausted { error_type, status },
        Some(429) => CopilotOutputError::RateLimited { error_type, status },
        Some(401 | 403) => CopilotOutputError::Auth { error_type, status },
        Some(code) if (500..600).contains(&code) => {
            CopilotOutputError::Network { error_type, status }
        }
        _ => {
            // No exhaustion status, but the text may still describe a
            // context/token limit — refine into TokenLimit when it does.
            if let Some(detail) =
                detect_token_limit(&error_type).or_else(|| detect_token_limit(stdout))
            {
                return CopilotOutputError::TokenLimit(detail);
            }
            CopilotOutputError::Reported { error_type, status }
        }
    }
}

/// Canonical subcommand for `subcommand: None`. Standalone `copilot` 1.0.49
/// has no `suggest` verb — the root command *is* the run — so this is empty.
const CANONICAL_SUBCOMMAND: &[&str] = &[];

/// GitHub Copilot CLI driver configuration.
#[derive(Debug, Clone)]
pub struct CopilotDriver {
    /// Binary name or path. Required.
    pub command: String,
    /// Print vs. interactive mode. Required.
    pub mode: AgentMode,
    /// Subcommand arguments inserted between the binary and the positional
    /// prompt. `None` falls back to the canonical subcommand (now empty);
    /// `Some(vec![])` also invokes the binary with no subcommand at all.
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
                let run = RunCommand {
                    options: RunOptions {
                        prompt: Some(prompt.as_str().to_owned()),
                        allow_all_tools: true,
                        ..RunOptions::default()
                    },
                    ..RunCommand::default()
                }
                .json();
                let mut process = Copilot::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&run);
                // Caller-supplied extra args follow the managed flags so they
                // can still override them.
                for arg in &self.args {
                    process.arg(arg);
                }
                apply_user_env(&mut process, &self.env);
                inject_agent_otel_resource_attrs(&mut process, path, "copilot");
                inject_copilot_trace_parent_env(&mut process);
                Ok(AgentCommand {
                    // The prompt is embedded in argv via `--prompt`, so no stdin.
                    process,
                    stdin: None,
                    io: StdioMode::Piped,
                })
            }
            AgentMode::Interactive => {
                let mut args = match &self.subcommand {
                    Some(sub) => sub.clone(),
                    None => CANONICAL_SUBCOMMAND
                        .iter()
                        .map(|arg| (*arg).to_owned())
                        .collect(),
                };
                args.extend(self.args.iter().cloned());
                let mut process = Copilot::new(&self.command)
                    .with_current_dir(path)
                    .to_process(&RunCommand::interactive_prompt(prompt.as_str()).with_args(args));
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
            // Adapter: project the crate's CLI-shaped result/error onto iter's
            // domain. `?` runs the `From<CopilotOutputError>` above.
            AgentMode::Headless => Ok(interpret_output(&raw)?),
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

    fn executable_read_paths(&self) -> Vec<std::path::PathBuf> {
        Copilot::new(&self.command).executable_read_paths()
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(args.contains(&"--prompt".to_owned()), "got {args:?}");
        assert!(args.contains(&"hello-copilot".to_owned()), "got {args:?}");
        assert!(
            args.contains(&"--allow-all-tools".to_owned()),
            "got {args:?}"
        );
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
        // The stale `gh copilot suggest` verb must never be injected.
        assert!(!args.contains(&"suggest".to_owned()), "got {args:?}");
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
        // Explicit empty subcommand invokes the standalone TUI binary bare.
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
    fn interactive_command_none_subcommand_injects_no_verb() {
        // With `subcommand: None`, iter's canonical subcommand is empty: no
        // `suggest` (or any other verb) is injected — only the prompt remains.
        let d = copilot_driver("copilot", AgentMode::Interactive);
        let prompt = Prompt::from("go");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        assert_eq!(args, vec!["go".to_owned()], "got {args:?}");
        assert!(!args.contains(&"suggest".to_owned()), "got {args:?}");
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
        assert!(args.contains(&"--prompt"), "got {args:?}");
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
