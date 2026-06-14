//! [`Agent`] trait — the AI agent that runs inside a
//! [`Workspace`](crate::workspace::Workspace).
//!
//! The trait is **dyn-compatible**: the runner drives a single agent through
//! a `Box<dyn Agent>` trait object (R18 — a closed set of agent kinds at the
//! definition layer, a trait object at run time, never both). To make `dyn
//! Agent` legal, [`run`](Agent::run) returns a boxed future via
//! [`async_trait`](async_trait::async_trait) — the same mechanism the
//! [`Workspace`](crate::workspace::Workspace) axis uses. The per-call boxing
//! allocation is irrelevant here: every `run` spawns a CLI subprocess that
//! runs for minutes, dwarfing one heap allocation and one indirect call.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::agent::command_path::CommandPath;
use crate::agent::{AgentError, AgentKind, AgentRun};
use crate::log::{NoopSink, OutputSink};
use crate::prompt::Prompt;
use crate::signal::{SignalId, SignalKind};

/// Per-iteration context handed to [`Agent::run`].
///
/// Bundles every piece of information the agent needs to execute one
/// iteration so the trait can grow new optional inputs (stdio sink, stdio
/// policy, …) without reshuffling every implementation's signature each
/// time. Constructed by the runner once per signal and consumed by the
/// agent for that single invocation.
///
/// The borrowed `'a` lifetime ties the context to the workspace path and
/// rendered prompt that live on the runner's stack frame for the duration
/// of one iteration; the agent must not retain the context past its
/// [`Agent::run`] call.
#[non_exhaustive]
pub struct AgentInvocation<'a> {
    /// Filesystem path of the workspace the agent should operate against.
    pub workspace_path: &'a Path,
    /// Rendered prompt for this iteration.
    pub prompt: &'a Prompt,
    /// Cancellation token honored by the agent (see [`Agent`] trait docs).
    pub cancel: CancellationToken,
    /// Identifier of the signal that triggered this iteration. Useful for
    /// agent-side logging and correlation against
    /// [`RunnerLifecycleEvent`](crate::runner::RunnerLifecycleEvent)
    /// events emitted around the same call.
    pub signal_id: SignalId,
    /// Kind of signal that triggered this iteration.
    pub signal_kind: SignalKind,
    /// Sink the agent should tee its child stdout/stderr through so every
    /// line lands in `log.ndjson`. Defaults to a [`NoopSink`] for tests
    /// and standalone constructions; the runner replaces it with the
    /// active `Arc<dyn OutputSink>` supplied by the caller when running
    /// under a process record.
    pub stdio_sink: Arc<dyn OutputSink>,
    /// Optional per-iteration timeout. When set, [`run_with_timeout`]
    /// cancels the agent after this duration and returns
    /// [`AgentError::IterationTimeout`](crate::agent::AgentError::IterationTimeout).
    pub iteration_timeout: Option<Duration>,
    /// Per-exploration hook isolation key. Distinguishes one Runner's
    /// stop-hook installation from another's when both explore the same
    /// workspace path. `"default"` for standalone `iter run`; the operator
    /// supplies a per-exploration value so that two explorations sharing a
    /// workspace path get separate hook-installation directories.
    pub hook_isolation_key: String,
    /// Argv prefix the agent's child command must be launched under for the
    /// active workspace's isolation to take effect.
    ///
    /// This is typed command-construction data, **not** an environment
    /// variable: the runner reads it from the active workspace via
    /// [`Workspace::sandbox_command_prefix`](crate::workspace::Workspace::sandbox_command_prefix)
    /// after setup and threads it here. Process-launch helpers splice it in
    /// front of the agent's own program/args. Empty for `local`/`clone`
    /// (non-sandbox) workspaces, in which case the command runs verbatim.
    pub sandbox_command_prefix: &'a [OsString],
    /// Operator-declared environment variables for the agent child.
    ///
    /// This is the same declared-env set the sandbox profile consumes while
    /// building confinement. Drivers apply it directly to the child command;
    /// sandbox backends also use it to carry those explicit values across
    /// clear-env boundaries. Host-inherited passthrough patterns remain
    /// separate on `SandboxProfile::env_pass`.
    pub declared_env: &'a [(String, String)],
}

impl std::fmt::Debug for AgentInvocation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInvocation")
            .field("workspace_path", &self.workspace_path)
            .field("prompt", &self.prompt)
            .field("cancel", &self.cancel)
            .field("signal_id", &self.signal_id)
            .field("signal_kind", &self.signal_kind)
            .field("stdio_sink", &"<dyn OutputSink>")
            .field("iteration_timeout", &self.iteration_timeout)
            .field("hook_isolation_key", &self.hook_isolation_key)
            .field("sandbox_command_prefix", &self.sandbox_command_prefix)
            .field(
                "declared_env",
                &self
                    .declared_env
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<'a> AgentInvocation<'a> {
    /// Construct a context for one iteration.
    #[must_use]
    pub fn new(
        workspace_path: &'a Path,
        prompt: &'a Prompt,
        cancel: CancellationToken,
        signal_id: SignalId,
    ) -> Self {
        Self {
            workspace_path,
            prompt,
            cancel,
            signal_id,
            signal_kind: SignalKind::Work,
            stdio_sink: Arc::new(NoopSink),
            iteration_timeout: None,
            hook_isolation_key: "default".to_owned(),
            sandbox_command_prefix: &[],
            declared_env: &[],
        }
    }

    /// Set the signal kind for this iteration.
    #[must_use]
    pub fn with_signal_kind(mut self, kind: SignalKind) -> Self {
        self.signal_kind = kind;
        self
    }

    /// Replace the [`OutputSink`] the agent will tee child output through.
    ///
    /// Returns `self` so the call can be chained after [`Self::new`].
    #[must_use]
    pub fn with_stdio_sink(mut self, sink: Arc<dyn OutputSink>) -> Self {
        self.stdio_sink = sink;
        self
    }

    /// Set the per-iteration timeout.
    #[must_use]
    pub fn with_iteration_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.iteration_timeout = timeout;
        self
    }

    /// Set the per-exploration hook isolation key (the operator supplies the
    /// value; `"default"` for standalone `iter run`).
    #[must_use]
    pub fn with_hook_isolation_key(mut self, key: String) -> Self {
        self.hook_isolation_key = key;
        self
    }

    /// Set the sandbox command prefix the agent's child must be launched
    /// under. The runner supplies the active workspace's
    /// [`sandbox_command_prefix`](crate::workspace::Workspace::sandbox_command_prefix);
    /// the borrow lives for the duration of one iteration. Defaults to an
    /// empty slice (the verbatim, non-sandbox case).
    #[must_use]
    pub fn with_sandbox_command_prefix(mut self, prefix: &'a [OsString]) -> Self {
        self.sandbox_command_prefix = prefix;
        self
    }

    /// Set the operator-declared child environment for this invocation.
    ///
    /// The runner supplies [`Agent::declared_env`] for the selected agent.
    /// Routers replace it when dispatching to a sub-agent so the direct
    /// process environment and sandbox clear-env reinjection see the same
    /// concrete set.
    #[must_use]
    pub fn with_declared_env(mut self, env: &'a [(String, String)]) -> Self {
        self.declared_env = env;
        self
    }
}

/// An AI agent that consumes a [`Prompt`] and produces an [`AgentRun`].
///
/// This is the **Agent level** of the three-layer agent stack (Command →
/// Driver/Adapter → Agent). An implementor is an Adapter: it drives a
/// per-CLI Command and projects that Command's rich, CLI-shaped result/error
/// down to iter's domain [`AgentRun`] / [`AgentError`].
///
/// An `Ok(AgentRun)` means **the agent ran**. A non-zero exit, a
/// signal, an in-band failure event, or a launch failure are all `Err` —
/// the caller never has to inspect a field inside `Ok` to learn the run
/// failed, and no raw CLI exit code leaks past the Adapter.
///
/// There is no `type Error` associated type: every driver uses
/// [`AgentError`]. The error vocabulary is fixed by iter's domain, not by
/// the driver.
///
/// # Cancellation
///
/// The context's `cancel` token fires when the runner is asked to shut
/// down (for example via `iter stop` → `SIGTERM`). Implementations
/// **must** treat an already-cancelled or mid-run cancellation as an
/// explicit request to stop the underlying process as quickly as possible
/// — typically by killing the child process and returning
/// [`AgentError::Cancelled`]. The runner itself does not wrap `run` in a
/// `select!`, because cooperative cancellation is the only way to give the
/// agent a chance to flush its output tail before exiting.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Run the agent for one iteration with the given context.
    ///
    /// The context is moved into the call by value because it bundles a
    /// fresh [`CancellationToken`] (cloned per iteration by the runner)
    /// plus borrows that only live for the duration of one iteration.
    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError>;

    /// Stable, human-meaningful label for this agent driver.
    ///
    /// Surfaced as the `iter.agent.name` telemetry attribute so a span names
    /// *which agent* ran (e.g. `"claude"`, `"codex"`, `"router"`) rather than
    /// a Rust type path. This is a **label**, not a discriminant —
    /// deliberately a `&'static str` on the `Agent` trait (mirroring
    /// [`Workspace::name`](crate::workspace::Workspace::name)), distinct in
    /// role from any agent-kind discriminant a later layer may add.
    ///
    /// The default returns a neutral placeholder and exists only so test-only
    /// stub agents need not state a name; every concrete driver overrides it,
    /// so the placeholder never reaches production telemetry.
    fn name(&self) -> &'static str {
        "agent"
    }

    /// The closed, object-safe discriminant of this agent.
    ///
    /// Distinct in role from [`name`](Agent::name): `name` is a telemetry
    /// *label*, whereas `kind` is the *discriminant* the sandbox layer keys
    /// per-agent OS-access policy off of. The sandbox layer matches
    /// **exhaustively** over [`AgentKind`] (see
    /// [`SandboxProfile::for_agent`](crate::workspace::sandbox::SandboxProfile::for_agent)),
    /// so every driver must report a kind — there is deliberately no default.
    fn kind(&self) -> AgentKind;

    /// Resolved on-disk location of this agent's configured binary, or
    /// `None` when the agent runs no external binary (or nothing on `$PATH`
    /// matches).
    ///
    /// Object-safe so the sandbox layer can grant read access to the
    /// executable image (and its canonical target behind a volta/nvm/asdf
    /// shim) through `&dyn Agent` without downcasting. The default returns
    /// `None` — only CLI-backed drivers whose binary must be mapped into a
    /// sandboxed child override it.
    fn command_path(&self) -> Option<CommandPath> {
        None
    }

    /// The named sub-agents this agent composes, in declaration order.
    ///
    /// Object-safe accessor backing the sandbox layer's `Router` match arm:
    /// a composite agent (the [`AgentRouter`](crate::agent::AgentRouter))
    /// returns its sub-agents so their profiles can be unioned. The default
    /// returns an empty slice — a leaf agent composes nothing.
    fn sub_agents(&self) -> &[(String, Box<dyn Agent>)] {
        &[]
    }

    /// Operator-declared environment variables for this agent's child
    /// command.
    ///
    /// These values are explicit child env settings, not host-inherited
    /// passthrough requests. The runner threads this slice into
    /// [`AgentInvocation`], and [`SandboxProfile::for_agent`](crate::workspace::sandbox::SandboxProfile::for_agent)
    /// snapshots it for sandbox setup so Linux `--clearenv` backends can
    /// restore it without relying on `env_pass`.
    fn declared_env(&self) -> &[(String, String)] {
        &[]
    }
}

/// Upper bound on how long the drain window waits for the agent future
/// after `iteration_timeout` fires. Derived from
/// [`AGENT_TERMINATION_GRACE`](super::process::AGENT_TERMINATION_GRACE)
/// so the drain always exceeds the SIGTERM grace period.
const ITERATION_TIMEOUT_DRAIN_GRACE: Duration =
    Duration::from_secs(super::process::AGENT_TERMINATION_GRACE.as_secs() + 5);

/// Run an agent with the optional iteration timeout from the context.
///
/// Creates a child cancellation token from `ctx.cancel` for the agent.
/// When `ctx.iteration_timeout` is `Some(limit)`, the agent future is
/// raced against the timeout. On expiry the child token is cancelled,
/// giving the agent up to [`ITERATION_TIMEOUT_DRAIN_GRACE`] to shut
/// down gracefully. During the drain window, the parent `ctx.cancel`
/// token is also watched so an operator Ctrl-C doesn't hang.
///
/// The agent future is pinned across the timeout boundary so it is
/// never dropped mid-flight — dropping would fire `ProcessGroup::Drop`
/// synchronously and bypass the agent's own graceful shutdown.
///
/// # Errors
///
/// Returns `AgentError` when the agent itself fails or when the
/// iteration timeout fires.
pub async fn run_with_timeout(
    agent: &dyn Agent,
    ctx: AgentInvocation<'_>,
) -> Result<AgentRun, AgentError> {
    let timeout = ctx.iteration_timeout;
    let parent_cancel = ctx.cancel.clone();
    let iter_cancel = parent_cancel.child_token();
    let agent_ctx = AgentInvocation {
        cancel: iter_cancel.clone(),
        ..ctx
    };
    match timeout {
        Some(limit) => {
            let mut agent_fut = std::pin::pin!(agent.run(agent_ctx));
            tokio::select! {
                biased;
                res = agent_fut.as_mut() => res,
                () = tokio::time::sleep(limit) => {
                    iter_cancel.cancel();
                    tokio::select! {
                        biased;
                        _ = agent_fut.as_mut() => {}
                        () = parent_cancel.cancelled() => {}
                        () = tokio::time::sleep(ITERATION_TIMEOUT_DRAIN_GRACE) => {}
                    }
                    Err(AgentError::IterationTimeout(limit))
                }
            }
        }
        None => agent.run(agent_ctx).await,
    }
}
