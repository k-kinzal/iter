//! [`Agent`] trait — the AI agent that runs inside a
//! [`Workspace`](crate::workspace::Workspace).
//!
//! Uses Return-Position-Impl-Trait-In-Trait (RPITIT) so that implementors
//! can write `async fn` bodies without paying for an extra allocation per
//! call. The associated future is required to be `Send` so it can be
//! polled by the multi-threaded `tokio` runtime.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agent::AgentReport;
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
pub struct AgentRunContext<'a> {
    /// Filesystem path of the workspace the agent should operate against.
    pub workspace_path: &'a Path,
    /// Rendered prompt for this iteration.
    pub prompt: &'a Prompt,
    /// Cancellation token honored by the agent (see [`Agent`] trait docs).
    pub cancel: CancellationToken,
    /// Identifier of the signal that triggered this iteration. Useful for
    /// agent-side logging and correlation against
    /// [`RunnerLifecycle`](crate::runner::RunnerLifecycle)
    /// events emitted around the same call.
    pub signal_id: SignalId,
    /// Kind of signal that triggered this iteration.
    pub signal_kind: SignalKind,
    /// Sink the agent should tee its child stdout/stderr through so every
    /// line lands in `log.ndjson`. Defaults to a [`NoopSink`] for tests
    /// and standalone constructions; the runner replaces it with the
    /// active `Arc<dyn OutputSink>` from the
    /// [`ProcessRuntime`](crate::process::ProcessRuntime) when running
    /// under a process record.
    pub stdio_sink: Arc<dyn OutputSink>,
    /// Optional per-iteration timeout. When set, [`run_with_timeout`]
    /// cancels the agent after this duration and returns
    /// [`AgentError::IterationTimeout`](crate::agent::AgentError::IterationTimeout).
    pub iteration_timeout: Option<Duration>,
    /// Compose service name for hook sidecar isolation. `"default"` for
    /// standalone `iter run`; set by `iter_compose` when running under a
    /// compose file so that two services sharing a workspace path get
    /// separate sidecar directories.
    pub service_name: String,
}

impl std::fmt::Debug for AgentRunContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunContext")
            .field("workspace_path", &self.workspace_path)
            .field("prompt", &self.prompt)
            .field("cancel", &self.cancel)
            .field("signal_id", &self.signal_id)
            .field("signal_kind", &self.signal_kind)
            .field("stdio_sink", &"<dyn OutputSink>")
            .field("iteration_timeout", &self.iteration_timeout)
            .field("service_name", &self.service_name)
            .finish()
    }
}

impl<'a> AgentRunContext<'a> {
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
            service_name: "default".to_owned(),
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

    /// Set the compose service name for hook sidecar isolation.
    #[must_use]
    pub fn with_service_name(mut self, name: String) -> Self {
        self.service_name = name;
        self
    }
}

/// An AI agent that consumes a [`Prompt`] and produces an [`AgentReport`].
///
/// # Cancellation
///
/// The context's `cancel` token fires when the runner is asked to shut
/// down (for example via `iter stop` → `SIGTERM`). Implementations
/// **must** treat an already-cancelled or mid-run cancellation as an
/// explicit request to stop the underlying process as quickly as possible
/// — typically by killing the child process and returning an error. The
/// runner itself does not wrap `run` in a `select!`, because cooperative
/// cancellation is the only way to give the agent a chance to flush its
/// output tail before exiting.
pub trait Agent: Send + Sync {
    /// Agent-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Run the agent for one iteration with the given context.
    ///
    /// The context is moved into the call by value because it bundles a
    /// fresh [`CancellationToken`] (cloned per iteration by the runner)
    /// plus borrows that only live for the duration of one iteration.
    fn run(
        &self,
        ctx: AgentRunContext<'_>,
    ) -> impl Future<Output = Result<AgentReport, Self::Error>> + Send;
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
pub async fn run_with_timeout<A>(
    agent: &A,
    ctx: AgentRunContext<'_>,
) -> Result<AgentReport, super::AgentError>
where
    A: Agent,
    A::Error: Into<super::AgentError>,
{
    let timeout = ctx.iteration_timeout;
    let parent_cancel = ctx.cancel.clone();
    let iter_cancel = parent_cancel.child_token();
    let agent_ctx = AgentRunContext {
        cancel: iter_cancel.clone(),
        ..ctx
    };
    match timeout {
        Some(limit) => {
            let mut agent_fut = std::pin::pin!(agent.run(agent_ctx));
            tokio::select! {
                biased;
                res = agent_fut.as_mut() => res.map_err(Into::into),
                () = tokio::time::sleep(limit) => {
                    iter_cancel.cancel();
                    tokio::select! {
                        biased;
                        _ = agent_fut.as_mut() => {}
                        () = parent_cancel.cancelled() => {}
                        () = tokio::time::sleep(ITERATION_TIMEOUT_DRAIN_GRACE) => {}
                    }
                    Err(super::AgentError::IterationTimeout(limit))
                }
            }
        }
        None => agent.run(agent_ctx).await.map_err(Into::into),
    }
}
