//! [`AgentDriver`] — the bidirectional translator for one agent CLI.
//!
//! A driver owns exactly the CLI-shaped knowledge of one agent CLI, in both
//! directions:
//!
//! - **Outbound** ([`command`](AgentDriver::command)): iter's intent (a
//!   prompt, a working path, a session token) → the CLI's dialect (argv,
//!   env, stdin payload, stdio shape), as an [`AgentCommand`] value.
//! - **Inbound** ([`interpret`](AgentDriver::interpret)): the CLI's dialect
//!   (its machine-readable output and exit status) → iter's vocabulary
//!   ([`AgentRun`] / [`AgentError`]): completion recognition, failure
//!   classification, session extraction.
//!
//! A driver never spawns, feeds, tees, or terminates a process — that is
//! the agent cycle, owned by [`Agent`](crate::agent::Agent). And it never
//! decides *where* the process runs — that is the workspace seam
//! ([`ActiveWorkspace::spawn`](crate::workspace::ActiveWorkspace::spawn)).
//!
//! The exit status is deliberately not the whole truth: CLIs report their
//! real verdict in their own dialect (Claude Code can exit `0` with
//! `is_error: true`; Codex signals success only via `turn.completed`), which
//! is why the inbound direction exists at all.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::prompt::Prompt;
use crate::workspace::StdioMode;

/// One assembled child-process invocation of an agent CLI — a thing, not an
/// act: the [`Agent`](crate::agent::Agent) performs the invoking.
///
/// # Invariant
///
/// When `io` is [`StdioMode::Inherit`] (interactive TUI), `stdin` must be
/// `None` — the child inherits the parent terminal and nothing can be fed.
pub struct AgentCommand {
    /// The prepared command: program, args, env, and any CLI-specific
    /// telemetry injection already applied. The working directory is set by
    /// the workspace at spawn time; a value set here is overwritten.
    pub process: tokio::process::Command,
    /// Input to feed on stdin after spawn (prompt-over-stdin CLIs), or
    /// `None` when the prompt is already embedded in the argv.
    pub stdin: Option<String>,
    /// How the child's stdio is wired: piped capture or terminal
    /// inheritance.
    pub io: StdioMode,
}

impl std::fmt::Debug for AgentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentCommand")
            .field("process", &self.process)
            .field("stdin", &self.stdin.as_ref().map(String::len))
            .field("io", &self.io)
            .finish()
    }
}

/// The bidirectional translator for one agent CLI. See the [module
/// docs](self) for the two directions and what deliberately lies outside
/// them.
///
/// Beyond the two translation methods, a driver exposes only **facts about
/// itself** — its closed discriminant, executable read allowances needed by
/// its child command, its declared child environment, its session-persistence
/// file. Each fact has a real consumer (the sandbox profile, or the agent
/// cycle); none walks into another object's composition.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// Outbound translation: assemble one child-process invocation.
    ///
    /// `path` is the working tree the child will run in (usable for
    /// path-derived arguments; the actual cwd is imposed at spawn).
    /// `session` is the resolved session token when this driver declared a
    /// [`session_file`](AgentDriver::session_file) — validation of its
    /// format is the driver's job.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when the invocation cannot be assembled —
    /// misconfiguration (empty command, invalid session token) maps to
    /// [`AgentError::Launch`].
    fn command(
        &self,
        path: &Path,
        prompt: &Prompt,
        session: Option<&str>,
    ) -> Result<AgentCommand, AgentError>;

    /// Inbound translation: read the CLI's own verdict out of the child's
    /// output and exit status.
    ///
    /// Recognises completion, classifies failures into iter's closed
    /// [`AgentError`] vocabulary (token-limit detection feeds routing), and
    /// extracts the session id. For [`StdioMode::Inherit`] runs the output
    /// carries empty stdout/stderr and only the exit status speaks.
    ///
    /// The agent's *value* never flows through here — it lives in the
    /// workspace's files and survives via teardown's apply-back. This method
    /// only judges the run's record.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when the run did not complete a successful
    /// turn, per this CLI's dialect.
    fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError>;

    /// CLI-specific preparation before the child is spawned (e.g. installing
    /// an interactive stop hook). Default: nothing to prepare.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when preparation fails;
    /// [`cleanup`](AgentDriver::cleanup) will **not** run in that case.
    async fn prepare(&self, _path: &Path) -> Result<(), AgentError> {
        Ok(())
    }

    /// CLI-specific cleanup after the run (e.g. restoring settings a hook
    /// install overwrote). Once [`prepare`](AgentDriver::prepare) has
    /// succeeded, the agent cycle guarantees this runs on **every** path —
    /// success, failure, and cancellation. Default: nothing to clean up.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when cleanup fails. A run error takes
    /// precedence; a cleanup error surfaces only when the run itself
    /// succeeded.
    async fn cleanup(&self, _path: &Path) -> Result<(), AgentError> {
        Ok(())
    }

    /// The closed, object-safe discriminant of this driver.
    ///
    /// The sandbox layer matches **exhaustively** over [`AgentKind`] (see
    /// [`SandboxProfile::for_drivers`](crate::workspace::sandbox::SandboxProfile::for_drivers)),
    /// so every driver must report a kind — there is deliberately no
    /// default. Also the source of the telemetry label
    /// ([`AgentKind::label`]).
    fn kind(&self) -> AgentKind;

    /// Files the sandbox must allow the child to read so its configured
    /// executable can be launched.
    ///
    /// For CLI-backed drivers this is usually the resolved binary path and,
    /// when the configured command is a symlink/shim, its canonical target.
    /// The executable lookup itself belongs to the CLI executor (or to a
    /// custom driver for driver-owned command vectors); the Agent boundary only
    /// sees the already-projected read allowances that the sandbox consumes.
    fn executable_read_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Operator-declared environment variables for this driver's child
    /// command. These are explicit child env settings, not host-inherited
    /// passthrough requests; the sandbox profile snapshots them so Linux
    /// clear-env confinement can restore them.
    fn declared_env(&self) -> &[(String, String)] {
        &[]
    }

    /// The file persisting this driver's session token across iterations,
    /// when session continuity is configured. The agent cycle resolves it
    /// (reading or generating the token) and passes the result to
    /// [`command`](AgentDriver::command) as `session`.
    fn session_file(&self) -> Option<&Path> {
        None
    }
}
