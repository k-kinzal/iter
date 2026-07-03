//! [`GrokDriver`] — Grok Build (xAI `grok`) CLI integration.
//!
//! # Headless-first
//!
//! Grok Build ships an official **headless mode** built for exactly the
//! automation use case iter targets. iter drives it through that path
//! only:
//!
//! ```text
//! grok -p "<prompt>" --always-approve --output-format json [-s <session-id>] [args...]
//! ```
//!
//! * `-p/--single <PROMPT>` sends one prompt and exits without entering the
//!   interactive UI — the prompt is the *value* of the flag, not a trailing
//!   positional. The single response is written to stdout; the Command level
//!   parses the `--output-format json` result object (see `command.rs`).
//! * `--always-approve` auto-approves tool executions. iter always runs the
//!   agent inside a `sandbox-exec` / `bwrap` profile that is the real
//!   filesystem boundary, and a detached runner has no tty to answer the
//!   CLI's own per-tool prompt — without this every tool call would stall
//!   waiting for an approval that can never arrive. It is emitted before
//!   user `args` so a caller can still append their own `--permission-mode`
//!   downstream if a future CLI revision prefers it.
//! * `-s/--session-id <ID>` is emitted only when [`GrokDriver::session_id_file`]
//!   is set. Grok's `-s` flag *creates or resumes* a named headless session,
//!   so passing the same id across iterations gives the agent continuous
//!   context — the narrowest exploration mode (see the field docs).
//!
//! Grok's TUI mode and its ACP (`grok agent stdio`) integration are out of
//! scope for this driver; the headless path covers iter's spawn-per-iteration
//! model without the project-local Stop-hook installation the TUI drivers need.
//!
//! # Authentication
//!
//! Headless `grok` authenticates with `XAI_API_KEY` (or a prior local
//! login). The sandbox profile passes `XAI_*` / `GROK_*` through; the
//! per-kind policy lives in the `Grok` arm of
//! [`SandboxProfile::for_agent`](crate::workspace::sandbox::SandboxProfile::for_agent).
//!
//! # Construction
//!
//! [`GrokDriver`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The driver is constructed directly from its fields.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::process::{RawOutput, apply_user_env};
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

mod command;

use command::{GrokCommand, GrokError};

impl From<GrokError> for AgentError {
    /// Adapter projection: collapse Grok Build's CLI-shaped error hierarchy
    /// onto iter's minimal domain error. Only [`GrokError::TokenLimit`] is
    /// router-relevant and preserved as [`AgentError::TokenLimit`]; the rest
    /// become the generic failure / signal variants.
    fn from(err: GrokError) -> Self {
        match err {
            GrokError::TokenLimit(detail) => Self::TokenLimit(detail),
            GrokError::Signal(sig) => Self::TerminatedBySignal(sig),
            GrokError::Reported { message, exit_code } => Self::Failed {
                code: exit_code,
                message: format!("grok reported an error result: {message}"),
            },
            GrokError::NoResult { exit_code } => Self::Failed {
                code: exit_code,
                message: "grok produced no terminal result".to_owned(),
            },
        }
    }
}

/// Grok Build CLI driver configuration.
#[derive(Debug, Clone)]
pub struct GrokDriver {
    /// Binary name or path. Required (no implicit `"grok"` fallback).
    pub command: String,
    /// Additional arguments appended after the iter-managed headless flags.
    pub args: Vec<String>,
    /// Optional path (relative to the workspace cwd, unless absolute) of a
    /// file that stores a stable Grok session id across iterations.
    ///
    /// When set, every invocation passes `-s <uuid>`:
    ///
    /// * If the file does not exist (or is empty), iter generates a fresh
    ///   v4 UUID, writes it to the path, and hands it to Grok. The `-s`
    ///   flag tells Grok to *create* a headless session with that id.
    /// * On every subsequent invocation iter reads the same file and passes
    ///   the same uuid, which tells Grok to *resume* the existing session —
    ///   giving the agent continuous context across iter iterations. This is
    ///   the narrowest exploration mode because accumulated agent context
    ///   keeps later turns close to earlier ones.
    ///
    /// Lifecycle (deleting the file to end an exploration run) is left to
    /// the caller — typically an `on workspace_teardown_finished` hook that
    /// drops the file on the final iteration. iter does not own that
    /// decision because it has no notion of "end of exploration".
    pub session_id_file: Option<PathBuf>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
}

fn home_subpath(leaf: &str) -> Option<PathBuf> {
    // Routes through the single core base-dir helper, which treats an empty
    // `$HOME` as unset (`None`) — intentional; do not revert to a raw
    // `var_os("HOME")` that would yield a bogus `"".join(leaf)`.
    crate::home::home_dir().map(|h| h.join(leaf))
}

impl GrokDriver {
    /// `${HOME}/.grok` — persistent configuration root and headless session
    /// state sink (sessions under `sessions/`). `None` when `HOME` is unset.
    #[must_use]
    pub fn home_dir() -> Option<PathBuf> {
        home_subpath(".grok")
    }

    /// `${HOME}/.grok/auth.json` — on-disk OAuth token store written by
    /// `grok login`. Headless runs that authenticate with `XAI_API_KEY`
    /// never touch it, but a browser-login operator needs it readable.
    /// `None` when `HOME` is unset.
    #[must_use]
    pub fn auth_path() -> Option<PathBuf> {
        Self::home_dir().map(|d| d.join("auth.json"))
    }

    /// `${HOME}/.grok/config.toml` — CLI settings file. `None` when `HOME`
    /// is unset.
    #[must_use]
    pub fn config_path() -> Option<PathBuf> {
        Self::home_dir().map(|d| d.join("config.toml"))
    }
}

#[async_trait]
impl AgentDriver for GrokDriver {
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        session: Option<&str>,
    ) -> Result<AgentCommand, AgentError> {
        // The session id is resolved by the agent cycle (async filesystem
        // work) and handed in as `session`; when `session_id_file` is unset
        // the cycle passes `None` and no `-s` flag is emitted.
        let mut process = GrokCommand {
            program: &self.command,
            prompt,
            args: &self.args,
            session_id: session,
        }
        .build(path);
        apply_user_env(&mut process, &self.env);
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted — a *verified negative* for `grok 0.2.45`, not an unknown:
        //
        // * `TRACEPARENT` / `TRACESTATE` are not consumed. The shipped binary
        //   contains no `TRACEPARENT`/`TRACESTATE` reference at all (string
        //   scan of `~/.local/bin/grok`), and headless mode documents only
        //   `XAI_API_KEY` / `GROK_HOME` / `GROK_LOG_FILE` / `RUST_LOG`
        //   (`~/.grok/docs/user-guide/14-headless-mode.md`). Grok starts its
        //   own trace; injecting a W3C carrier would make iter's trace *look*
        //   correlated without Grok joining it.
        // * `OTEL_RESOURCE_ATTRIBUTES` / `OTEL_SERVICE_NAME` are not honored.
        //   Probing a headless run with `OTEL_EXPORTER_OTLP_ENDPOINT` aimed at
        //   a local collector showed Grok *does* export OTLP, but every span
        //   carried Grok's own Resource (`service.name=grok-cli`,
        //   `app.entrypoint=headless`, Grok `user.id`/`team.id`) — the
        //   injected `OTEL_SERVICE_NAME` / `iter.*` attributes were dropped.
        //   That export is Grok's private telemetry pipeline (default
        //   `cli-chat-proxy.grok.com`, gated by `GROK_TELEMETRY_TRACE_UPLOAD`),
        //   so `inject_agent_otel_resource_attrs` would attach iter's
        //   signal/workspace attributes to nothing Grok emits. Repointing
        //   `OTEL_EXPORTER_OTLP_ENDPOINT` is likewise avoided — it would hijack
        //   Grok's own telemetry destination, not correlate iter's trace.
        //
        // Re-verify against a newer CLI before enabling either injection.

        // The prompt is the value of `-p` (delivered inline), so no stdin.
        Ok(AgentCommand {
            process,
            stdin: None,
            io: StdioMode::Piped,
        })
    }

    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<GrokError>` impl above.
        let result = command::interpret(RawOutput::from(output))?;
        // Only `session_id` crosses into the domain `AgentRun`. The rich
        // record (`request_id`, `thought`, `stop_reason`, `usage`) stays at the
        // Command layer: `AgentRun` carries only what a Factor consumes, and
        // iter has no agreed token/cost Factor field — matching how the
        // Cursor/Claude drivers keep their usage/cost out of `AgentRun`. (Moot
        // for `grok 0.2.45`, which reports no usage/cost anyway.)
        Ok(AgentRun {
            session_id: result.session_id,
        })
    }

    fn kind(&self) -> AgentKind {
        AgentKind::Grok
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    ///
    /// The returned handle exposes both the resolved path and its canonical
    /// target so the sandbox layer can grant read access to a symlink shim
    /// (volta, nvm, asdf, homebrew cask).
    fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }

    fn declared_env(&self) -> &[(String, String)] {
        &self.env
    }

    fn session_file(&self) -> Option<&Path> {
        self.session_id_file.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::process::RawExit;
    use crate::agent::testutil::{drive_capturing, fake_binary_script};
    use tempfile::TempDir;
    use tokio::fs;

    fn driver(command: impl Into<String>) -> GrokDriver {
        GrokDriver {
            command: command.into(),
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
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

    fn synth_output(exit: RawExit, stdout: &str) -> std::process::Output {
        std::process::Output {
            status: exit.into_exit_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    // ----- command(): outbound translation ---------------------------------

    #[test]
    fn command_passes_prompt_as_value_of_p_flag() {
        let d = driver("grok");
        let prompt = Prompt::from("hello-grok");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let args = argv(&command);
        // The prompt is the value of `-p`, immediately following it.
        let pos = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[pos + 1], "hello-grok", "got {args:?}");
        assert_eq!(command.stdin, None, "prompt is inline, not stdin");
        assert_eq!(command.io, StdioMode::Piped);
    }

    #[test]
    fn command_emits_always_approve_and_json_format() {
        let d = driver("grok");
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(
            args.contains(&"--always-approve".to_owned()),
            "got {args:?}"
        );
        assert!(args.contains(&"--output-format".to_owned()), "got {args:?}");
        assert!(args.contains(&"json".to_owned()), "got {args:?}");
    }

    #[test]
    fn command_without_session_emits_no_session_flag() {
        let d = driver("grok");
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(!args.contains(&"-s".to_owned()), "got {args:?}");
    }

    #[test]
    fn command_with_session_emits_session_flag() {
        let d = driver("grok");
        let prompt = Prompt::from("x");
        let args = argv(
            &d.command(Path::new("."), &prompt, Some("sess-x"))
                .expect("command"),
        );
        let pos = args.iter().position(|a| a == "-s").expect("-s present");
        assert_eq!(args[pos + 1], "sess-x", "got {args:?}");
    }

    #[test]
    fn extra_args_are_forwarded_after_managed_flags() {
        let mut d = driver("grok");
        d.args = vec!["--model".into(), "grok-2".into()];
        let prompt = Prompt::from("x");
        let args = argv(&d.command(Path::new("."), &prompt, None).expect("command"));
        assert!(args.contains(&"--model".to_owned()), "got {args:?}");
        assert!(args.contains(&"grok-2".to_owned()), "got {args:?}");
    }

    #[test]
    fn declared_env_is_set_on_the_command() {
        let mut d = driver("grok");
        d.env = vec![("GROK_TEST_ENV_VAR".into(), "env-value".into())];
        let prompt = Prompt::from("x");
        let command = d.command(Path::new("."), &prompt, None).expect("command");
        let has = command.process.as_std().get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("GROK_TEST_ENV_VAR")
                && v == Some(std::ffi::OsStr::new("env-value"))
        });
        assert!(has, "declared env must be applied to the child command");
    }

    // ----- interpret(): inbound projection onto the domain ------------------

    #[test]
    fn interpret_verified_result_extracts_session_id() {
        let d = driver("grok");
        let body = r#"{"text":"OK","stopReason":"EndTurn","sessionId":"sess-x","requestId":"r"}"#;
        let run = d
            .interpret(&synth_output(RawExit::Code(0), body))
            .expect("ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
    }

    #[test]
    fn interpret_type_error_object_maps_to_failed() {
        let d = driver("grok");
        let body = r#"{"type":"error","message":"auth failed"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn interpret_token_limit_in_error_maps_to_token_limit() {
        let d = driver("grok");
        let body = r#"{"error":"Error: context window exceeded"}"#;
        let err = d
            .interpret(&synth_output(RawExit::Code(1), body))
            .expect_err("must fail");
        assert!(matches!(err, AgentError::TokenLimit(_)), "got {err:?}");
    }

    // ----- through the full cycle -------------------------------------------

    /// Fake `grok` binary: echoes each argv arg to *stderr* (so the capture
    /// sink can observe the flags and the values following them), then prints
    /// a valid headless result JSON object to stdout so [`command::interpret`]
    /// parses an `Ok`. Uses the verified `grok 0.2.45` shape
    /// (`text`/`stopReason`) so these integration tests exercise the primary
    /// parse path, not the legacy fallback.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"sessionId":"sess-x","text":"ok","stopReason":"EndTurn"}'"#;

    #[tokio::test]
    async fn headless_passes_prompt_and_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("hello-grok");
        let dir = TempDir::new().expect("tmp");
        let (result, sink) = drive_capturing(d, dir.path(), &prompt).await;
        let run = result.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let mut lines = echoed.lines();
        // First emitted arg must be `-p`, immediately followed by the prompt.
        assert_eq!(lines.next(), Some("-p"), "argv was: {echoed:?}");
        assert_eq!(lines.next(), Some("hello-grok"), "argv was: {echoed:?}");
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--always-approve"), "got {args:?}");
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn without_session_id_file_emits_no_session_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let tmp = TempDir::new().expect("tmp");
        let d = driver(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
        result.expect("run ok");
        assert!(
            !sink.stderr().await.lines().any(|l| l == "-s"),
            "unset session_id_file must not emit -s",
        );
    }

    /// Extract the uuid emitted after `-s` in the captured argv.
    fn session_id_from_argv(echoed: &str) -> Option<String> {
        let mut lines = echoed.lines();
        while let Some(line) = lines.next() {
            if line == "-s" {
                return lines.next().map(str::to_string);
            }
        }
        None
    }

    #[tokio::test]
    async fn generates_and_writes_session_id_on_first_run() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let tmp = TempDir::new().expect("tmp");
        let mut d = driver(bin.to_string_lossy());
        d.session_id_file = Some(PathBuf::from(".iter/session-id"));

        let prompt = Prompt::from("x");
        let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
        result.expect("run ok");

        let emitted_uuid =
            session_id_from_argv(&sink.stderr().await).expect("-s <uuid> must appear in argv");
        let parsed =
            uuid::Uuid::parse_str(&emitted_uuid).expect("emitted session id must parse as uuid");
        assert_eq!(parsed.get_version_num(), 4, "must be a v4 uuid");

        let file = tmp.path().join(".iter").join("session-id");
        let persisted = fs::read_to_string(&file).await.expect("read session id");
        assert_eq!(persisted.trim(), emitted_uuid);
    }

    #[tokio::test]
    async fn reuses_existing_session_id_file() {
        let tmp = TempDir::new().expect("tmp");
        let fixed = "11111111-2222-4333-8444-555555555555";
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), format!("{fixed}\n"))
            .await
            .expect("seed session id");

        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
            let mut d = driver(bin.to_string_lossy());
            d.session_id_file = Some(PathBuf::from(".iter/session-id"));
            let (result, sink) = drive_capturing(d, tmp.path(), &prompt).await;
            result.expect("run ok");
            assert_eq!(
                session_id_from_argv(&sink.stderr().await).as_deref(),
                Some(fixed),
                "must reuse seeded uuid",
            );
        }
        let persisted = fs::read_to_string(tmp.path().join(".iter/session-id"))
            .await
            .expect("read");
        assert_eq!(persisted.trim(), fixed, "seeded file must not be mutated");
    }
}
