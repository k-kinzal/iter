//! [`GrokAgent`] — Grok Build (xAI `grok`) CLI integration.
//!
//! # Headless-first
//!
//! Grok Build ships an official **headless mode** built for exactly the
//! automation use case iter targets. iter drives it through that path
//! only:
//!
//! ```text
//! grok -p "<prompt>" --always-approve [-s <session-id>] [args...]
//! ```
//!
//! * `-p/--single <PROMPT>` sends one prompt and exits without entering the
//!   interactive UI — the prompt is the *value* of the flag, not a trailing
//!   positional. The single response is written to stdout and captured into
//!   [`AgentReport::last_output`].
//! * `--always-approve` auto-approves tool executions. iter always runs the
//!   agent inside a `sandbox-exec` / `bwrap` profile that is the real
//!   filesystem boundary, and a detached runner has no tty to answer the
//!   CLI's own per-tool prompt — without this every tool call would stall
//!   waiting for an approval that can never arrive. It is emitted before
//!   user `args` so a caller can still append their own `--permission-mode`
//!   downstream if a future CLI revision prefers it.
//! * `-s/--session-id <ID>` is emitted only when [`GrokSettings::session_id_file`]
//!   is set. Grok's `-s` flag *creates or resumes* a named headless session,
//!   so passing the same id across iterations gives the agent continuous
//!   context — the narrowest exploration mode (see the field docs).
//!
//! Grok's TUI mode and its ACP (`grok agent stdio`) integration are out of
//! scope for this driver; the headless path covers iter's spawn-per-iteration
//! model without the project-local Stop-hook machinery the TUI drivers need.
//!
//! # Authentication
//!
//! Headless `grok` authenticates with `XAI_API_KEY` (or a prior local
//! login). The sandbox profile passes `XAI_*` / `GROK_*` through; see
//! [`crate::workspace::sandbox::agent_requirements::grok`].
//!
//! # Construction
//!
//! [`GrokAgent`] exposes no defaults. Every field on [`GrokSettings`] is
//! required because the value is a project-shaped decision iter cannot
//! honestly pick on the operator's behalf.

use std::path::{Path, PathBuf};

use crate::{Agent, AgentReport, AgentRunContext, Prompt};
use tokio::process::Command;

use crate::agent::AgentError;
use crate::agent::process::{PromptDelivery, apply_user_env, detect_token_limit, run_command};
use crate::agent::session::SessionIdFile;

/// Fully-specified configuration for [`GrokAgent`].
///
/// Every field is required; there is no `Default` impl because every value
/// is a project-shaped decision the Iterfile must spell out explicitly.
#[derive(Debug, Clone)]
pub struct GrokSettings {
    /// Binary name or absolute path passed to
    /// [`tokio::process::Command::new`]. Name-only strings are resolved
    /// via `PATH`.
    pub command: String,
    /// Additional arguments appended after the iter-managed headless flags.
    /// Empty is allowed and the common case.
    pub args: Vec<String>,
    /// Optional path (relative to the workspace cwd, unless absolute) of a
    /// file that stores a stable Grok session id across iterations.
    /// `None` means "no session persistence"; `Some` means "persist the
    /// session id at this path".
    pub session_id_file: Option<PathBuf>,
    /// User-declared environment variables passed to the child process.
    pub env: Vec<(String, String)>,
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
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(leaf))
}

impl GrokAgent {
    /// Build a fully-specified Grok Build agent. Every knob must be decided
    /// by the caller; iter provides no implicit defaults.
    #[must_use]
    pub fn new(settings: GrokSettings) -> Self {
        let GrokSettings {
            command,
            args,
            session_id_file,
            env,
        } = settings;
        Self {
            command,
            args,
            session_id_file,
            env,
        }
    }

    /// Resolved on-disk location of the configured binary, or `None` when
    /// nothing on `$PATH` or the supplied path matches an existing file.
    ///
    /// The returned handle exposes both the resolved path and its canonical
    /// target for sandbox-layer consumers that need to grant read access to
    /// a symlink shim (volta, nvm, asdf, homebrew cask).
    #[must_use]
    pub fn command_path(&self) -> Option<crate::agent::command_path::CommandPath> {
        crate::agent::command_path::CommandPath::resolve(&self.command)
    }

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

    fn build_command(&self, path: &Path, prompt: &Prompt, session_id: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.current_dir(path);
        // `-p <prompt>` is the headless trigger: the prompt is the *value*
        // of the flag. Delivered inline (no stdin) — see `run` below.
        cmd.arg("-p").arg(prompt.as_str());
        cmd.arg("--always-approve");
        if let Some(sid) = session_id {
            cmd.arg("-s").arg(sid);
        }
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }
}

impl Agent for GrokAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        let AgentRunContext {
            workspace_path,
            prompt,
            cancel,
            stdio_sink,
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

        let mut command = self.build_command(workspace_path, prompt, session_id.as_deref());
        apply_user_env(&mut command, &self.env);
        // OTel trace-context / resource-attribute injection is deliberately
        // omitted: Grok Build's consumption of `TRACEPARENT` /
        // `OTEL_RESOURCE_ATTRIBUTES` is unverified, so — like the other
        // print-only drivers — iter does not make its traces *look*
        // correlated without confirming the agent actually participates.

        let report = run_command(command, PromptDelivery::Inline, cancel, stdio_sink).await?;
        if !report.exit_status.is_success()
            && let Some(detail) = report.last_output.as_deref().and_then(detect_token_limit)
        {
            return Err(AgentError::TokenLimit(detail));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::{ctx, fake_binary_script};
    use tempfile::TempDir;
    use tokio::fs;

    fn settings(command: impl Into<String>) -> GrokSettings {
        GrokSettings {
            command: command.into(),
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        }
    }

    /// Fake `grok` binary that echoes its argv (one arg per line) so tests
    /// can grep for flags and the values that follow them.
    const FAKE_ARGV_SCRIPT: &str = r#"for a in "$@"; do printf '%s\n' "$a"; done"#;

    #[tokio::test]
    async fn headless_passes_prompt_as_value_of_p_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let agent = GrokAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("hello-grok");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        let out = report.last_output.expect("last_output");
        let mut lines = out.lines();
        // First emitted arg must be `-p`, immediately followed by the prompt.
        assert_eq!(lines.next(), Some("-p"), "argv was: {out:?}");
        assert_eq!(lines.next(), Some("hello-grok"), "argv was: {out:?}");
    }

    #[tokio::test]
    async fn headless_emits_always_approve_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let agent = GrokAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(
            out.lines().any(|l| l == "--always-approve"),
            "headless mode must auto-approve tool executions, got {out:?}",
        );
    }

    #[tokio::test]
    async fn extra_args_are_forwarded_after_managed_flags() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let mut s = settings(bin.to_string_lossy());
        s.args = vec!["--output-format".into(), "json".into()];
        let agent = GrokAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(out.lines().any(|l| l == "--output-format"), "got {out:?}");
        assert!(out.lines().any(|l| l == "json"), "got {out:?}");
    }

    #[tokio::test]
    async fn env_is_forwarded_to_child() {
        let (_guard, bin) = fake_binary_script("printf '%s' \"$GROK_TEST_ENV_VAR\"");
        let mut s = settings(bin.to_string_lossy());
        s.env = vec![("GROK_TEST_ENV_VAR".into(), "env-value".into())];
        let agent = GrokAgent::new(s);
        let prompt = Prompt::from("x");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.last_output.expect("last_output"), "env-value");
    }

    // -----------------------------------------------------------------
    // session_id_file: continuous-context persistence across iterations.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn without_session_id_file_emits_no_session_flag() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        let agent = GrokAgent::new(settings(bin.to_string_lossy()));
        let prompt = Prompt::from("x");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");
        assert!(
            !out.lines().any(|l| l == "-s"),
            "unset session_id_file must not emit -s, got {out:?}",
        );
    }

    #[tokio::test]
    async fn generates_and_writes_session_id_on_first_run() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        let mut s = settings(bin.to_string_lossy());
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = GrokAgent::new(s);

        let prompt = Prompt::from("x");
        let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        let out = report.last_output.expect("last_output");

        // Extract the uuid that followed `-s` in argv.
        let mut lines = out.lines();
        let mut emitted_uuid: Option<String> = None;
        while let Some(line) = lines.next() {
            if line == "-s" {
                emitted_uuid = lines.next().map(str::to_string);
                break;
            }
        }
        let emitted_uuid = emitted_uuid.expect("-s <uuid> must appear in argv");
        let parsed =
            uuid::Uuid::parse_str(&emitted_uuid).expect("emitted session id must parse as uuid");
        assert_eq!(parsed.get_version_num(), 4, "must be a v4 uuid");

        let file = tmp.path().join(".iter").join("session-id");
        let persisted = fs::read_to_string(&file).await.expect("read session id");
        assert_eq!(persisted.trim(), emitted_uuid);
    }

    #[tokio::test]
    async fn reuses_existing_session_id_file() {
        let (_guard, bin) = fake_binary_script(FAKE_ARGV_SCRIPT);
        let tmp = TempDir::new().expect("tmp");
        let fixed = "11111111-2222-4333-8444-555555555555";
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), format!("{fixed}\n"))
            .await
            .expect("seed session id");

        let mut s = settings(bin.to_string_lossy());
        s.session_id_file = Some(PathBuf::from(".iter/session-id"));
        let agent = GrokAgent::new(s);

        let prompt = Prompt::from("x");
        for _ in 0..2 {
            let report = agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
            let out = report.last_output.expect("last_output");
            assert!(out.lines().any(|l| l == "-s"), "must emit -s, got {out:?}");
            assert!(out.lines().any(|l| l == fixed), "must reuse {fixed}, got {out:?}");
        }
        let persisted = fs::read_to_string(tmp.path().join(".iter/session-id"))
            .await
            .expect("read");
        assert_eq!(persisted.trim(), fixed, "seeded file must not be mutated");
    }
}
