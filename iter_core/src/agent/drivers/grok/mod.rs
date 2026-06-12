//! [`GrokAgent`] — Grok Build (xAI `grok`) CLI integration.
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
//! * `-s/--session-id <ID>` is emitted only when [`GrokAgent::session_id_file`]
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
//! [`GrokAgent`] exposes no defaults. Every field is required because the
//! value is a project-shaped decision iter cannot honestly pick on the
//! operator's behalf. The agent is constructed directly from its fields.

use std::path::PathBuf;

use crate::{Agent, AgentInvocation, AgentRun};
use async_trait::async_trait;

mod command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, apply_user_env, spawn_capture};
use crate::agent::session::SessionIdFile;
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

/// Grok Build CLI agent configuration.
#[derive(Debug, Clone)]
pub struct GrokAgent {
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

impl GrokAgent {
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
impl Agent for GrokAgent {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn kind(&self) -> crate::agent::AgentKind {
        crate::agent::AgentKind::Grok
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

    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        let AgentInvocation {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
            sandbox_command_prefix,
            ..
        } = ctx;
        // Resolve the session id *before* spawning so a filesystem failure
        // here surfaces as an `AgentError` instead of a confusing child
        // startup error. When `session_id_file` is unset this is a no-op and
        // no `-s` flag is emitted.
        let session_id = match &self.session_id_file {
            Some(file) => Some(
                SessionIdFile::new(file.clone())
                    .resolve(workspace_path)
                    .await?,
            ),
            None => None,
        };

        let mut command = GrokCommand {
            program: &self.command,
            prompt,
            args: &self.args,
            session_id: session_id.as_deref(),
        }
        .build(workspace_path);
        apply_user_env(&mut command, &self.env);
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
        let output = spawn_capture(
            command,
            PromptDelivery::Inline,
            cancel,
            stdio_sink,
            sandbox_command_prefix,
        )
        .await?;
        // Adapter: project the Command's CLI-shaped result/error onto iter's
        // domain. `?` runs the `From<GrokError>` impl above.
        let result = command::interpret(&output)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Prompt;
    use crate::agent::testutil::{ctx_capturing, fake_binary_script};
    use std::path::Path;
    use tempfile::TempDir;
    use tokio::fs;

    fn grok_agent(command: impl Into<String>) -> GrokAgent {
        GrokAgent {
            command: command.into(),
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    /// Fake `grok` binary: echoes each argv arg to *stderr* (so a
    /// [`CaptureSink`] can observe the flags and the values following them),
    /// then prints a valid headless result JSON object to stdout so
    /// [`command::interpret`] parses an `Ok`. Uses the verified `grok 0.2.45`
    /// shape (`text`/`stopReason`) so these integration tests exercise the
    /// primary parse path, not the legacy fallback.
    const FAKE_JSON_OK: &str = r#"for a in "$@"; do printf '%s\n' "$a" 1>&2; done
printf '%s' '{"sessionId":"sess-x","text":"ok","stopReason":"EndTurn"}'"#;

    #[tokio::test]
    async fn headless_passes_prompt_as_value_of_p_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = grok_agent(bin.to_string_lossy());
        let prompt = Prompt::from("hello-grok");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        let run = agent.run(ctx).await.expect("run ok");
        assert_eq!(run.session_id.as_deref(), Some("sess-x"));
        let echoed = sink.stderr().await;
        let mut lines = echoed.lines();
        // First emitted arg must be `-p`, immediately followed by the prompt.
        assert_eq!(lines.next(), Some("-p"), "argv was: {echoed:?}");
        assert_eq!(lines.next(), Some("hello-grok"), "argv was: {echoed:?}");
    }

    #[tokio::test]
    async fn headless_emits_always_approve_and_json_format() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let agent = grok_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(
            args.contains(&"--always-approve"),
            "headless mode must auto-approve tool executions, got {args:?}",
        );
        assert!(args.contains(&"--output-format"), "got {args:?}");
        assert!(args.contains(&"json"), "got {args:?}");
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_after_managed_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let mut s = grok_agent(bin.to_string_lossy());
        s.args = vec!["--model".into(), "grok-2".into()];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        let echoed = sink.stderr().await;
        let args: Vec<&str> = echoed.lines().collect();
        assert!(args.contains(&"--model"), "got {args:?}");
        assert!(args.contains(&"grok-2"), "got {args:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        // Echo the env var to stderr, then emit valid JSON to stdout.
        let script =
            "printf 'ENV=%s\\n' \"$GROK_TEST_ENV_VAR\" 1>&2\nprintf '%s' '{\"sessionId\":\"s\"}'";
        let (_guard, bin) = fake_binary_script(script);
        let mut s = grok_agent(bin.to_string_lossy());
        s.env = vec![("GROK_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = s;
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(Path::new("."), &prompt);
        agent.run(ctx).await.expect("run ok");
        assert!(sink.stderr().await.contains("ENV=env-value"));
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn without_session_id_file_emits_no_session_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
        let tmp = TempDir::new().expect("tmp");
        let agent = grok_agent(bin.to_string_lossy());
        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");
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
        let mut s = grok_agent(bin.to_string_lossy());
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = s;

        let prompt = Prompt::from("x");
        let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
        agent.run(ctx).await.expect("run ok");

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

        let mut s = grok_agent("placeholder");
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));

        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let (_guard, bin) = fake_binary_script(FAKE_JSON_OK);
            s.command = bin.to_string_lossy().into_owned();
            let agent = s.clone();
            let (ctx, sink) = ctx_capturing(tmp.path(), &prompt);
            agent.run(ctx).await.expect("run ok");
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
