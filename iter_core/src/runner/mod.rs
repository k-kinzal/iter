//! [`Runner`] — drives the per-signal stages that connect a [`Queue`],
//! a [`Workspace`], and an [`Agent`].
//!
//! The runner exposes a single `run` method that returns once one of the
//! configured termination conditions fires (or the supplied
//! [`CancellationToken`] is triggered).

pub mod builder;
pub mod config;
/// Error types for [`Runner::run`].
pub mod error;
pub mod event;
pub mod event_emitter;
pub mod event_handler;
mod events;
pub mod shell_event_handler;
pub mod iteration;
pub mod lifecycle;
pub mod observer;

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, field};

use crate::agent::Agent;
use crate::agent::AgentOutcomeKind;
use crate::prompt::{Prompt, PromptSelector};
use crate::queue::Queue;
use crate::signal::{Signal, SignalId};
use crate::workspace::Workspace;

pub use builder::{BuilderError, RunnerBuilder};
pub use config::{RunnerBehavior, RunnerConfig, RunnerSummary, RunnerTerminationReason};
pub use error::RunnerExitError;
pub use event::{ErrorStage, Event, EventName};
pub use event_emitter::{EmitReport, EventEmitter};
pub use event_handler::{BoxError, EventHandler};
pub use iteration::{IterationContext, IterationState, PreviousOutcome};
pub use lifecycle::{RedactedMetadata, RunnerLifecycle};
pub use shell_event_handler::ShellEventHandler;
pub use observer::{DynRunnerObserver, ObserveFuture, RunnerObserver};

use events::RunnerEvents;

/// Drives a queue of signals through a workspace and agent.
///
/// The runner is consumed by [`Runner::run`] so that owned state can be
/// moved into the loop.
///
/// `queue` is `Option`: a runner configured with `behavior = loop` may
/// operate without a queue, synthesising signals on each iteration. The
/// builder rejects the inconsistent `(queue=None, behavior=Wait)`
/// combination.
pub struct Runner<Q: Queue, W: Workspace, A: Agent> {
    pub(crate) queue: Option<Arc<Q>>,
    pub(crate) workspaces: Arc<dyn Fn() -> W + Send + Sync>,
    pub(crate) agent: A,
    pub(crate) prompt_selector: PromptSelector,
    pub(crate) events: EventEmitter,
    pub(crate) config: RunnerConfig,
    /// System-contract observer fan-out.
    ///
    /// Each registered observer receives the
    /// [`RunnerLifecycle`] projection of every per-step `Event` *before*
    /// the user-defined `events` emitter sees it. Observer errors are
    /// tallied separately into
    /// [`RunnerSummary::observer_error_count`]; they never block
    /// runner progress.
    pub(crate) observers: Vec<Arc<dyn DynRunnerObserver>>,
    /// Sink the agent should tee its child stdout/stderr through. Wired
    /// by [`RunnerBuilder::stdio_sink`] from
    /// [`crate::process::ProcessRuntime::stdio`]; unset runners get a
    /// [`crate::process::stdio::NoopSink`].
    pub(crate) stdio_sink: Arc<dyn crate::process::stdio::StdioSink>,
}

impl<Q, W, A> Runner<Q, W, A>
where
    Q: Queue + 'static,
    W: Workspace + 'static,
    A: Agent + 'static,
{
    /// Start a fluent [`RunnerBuilder`].
    pub fn builder() -> RunnerBuilder<Q, W, A> {
        RunnerBuilder::new()
    }

    /// Drive the runner loop.
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// The loop terminates when:
    ///
    /// * the supplied [`CancellationToken`] is fired,
    /// * the queue is drained (`dequeue` returns `Ok(None)`) — only when
    ///   the runner has a queue and `behavior = wait`,
    /// * `once` is set in [`RunnerConfig`] and one signal was processed, or
    /// * a runner stage error occurs and `continue_on_error` is `false`.
    ///
    /// When `behavior = loop` is configured the runner synthesises a
    /// signal each time the queue is empty (or whenever it has no queue),
    /// applying the configured `delay` between successive synthesised
    /// iterations. The first iteration runs without delay so a one-shot
    /// `behavior = loop` invocation starts immediately.
    pub async fn run(self, cancel: CancellationToken) -> Result<RunnerSummary, RunnerExitError>
    where
        A::Error: Into<crate::agent::AgentError>,
    {
        let Runner {
            queue,
            workspaces: workspace_factory,
            agent,
            prompt_selector,
            events: emitter,
            config,
            observers,
            stdio_sink,
        } = self;
        let ctx = RunCtx {
            queue: queue.as_deref(),
            workspace_factory: &*workspace_factory,
            agent: &agent,
            prompt_selector: &prompt_selector,
            config: &config,
            cancel: &cancel,
            stdio_sink: &stdio_sink,
        };
        let mut events = RunnerEvents::new(emitter, observers);
        let runner_started_at = Utc::now();
        let mut iter_state = IterationState::new(runner_started_at);
        let mut iteration_count: u32 = 0;
        let mut last_signal_id: Option<SignalId> = None;

        events.bootstrap(runner_started_at).await;
        let bootstrap_snapshot = iter_state.snapshot(0);
        events.runner_starting(&bootstrap_snapshot).await;

        let outcome = run_loop(
            &ctx,
            &mut events,
            &mut iter_state,
            &mut iteration_count,
            &mut last_signal_id,
        )
        .await;

        let (final_reason, final_iter_count) = match &outcome {
            Ok(s) => (s.termination_reason.clone(), s.iteration_count),
            Err(err) => (
                RunnerTerminationReason::Error {
                    stage: err.stage(),
                    message: err.message().to_owned(),
                },
                iteration_count,
            ),
        };
        let runner_finished_snapshot = iter_state.snapshot(final_iter_count);
        events
            .runner_finished(final_reason, final_iter_count, &runner_finished_snapshot)
            .await;

        with_counters(
            outcome,
            events.handler_error_count,
            events.observer_error_count,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Composition primitives for `Runner::run`.
//
// `run` is a workflow over per-signal stages.  Each concern below holds
// exactly one responsibility: `RunCtx` carries the immutable execution
// context; `RunnerEvents` (in events.rs) owns the broadcast + tally
// pair; `ProcessingFailure` and `NextSignal` are the data shapes that
// compose stage outcomes; `decide_after_processing_failure` is the pure
// failure-policy decision; the `next_signal` / `render_prompt` /
// `drive_workspace` stage functions are typed and side-effect-explicit.
// ─────────────────────────────────────────────────────────────────────────

struct RunCtx<'a, Q: Queue, W: Workspace, A: Agent> {
    queue: Option<&'a Q>,
    workspace_factory: &'a (dyn Fn() -> W + Send + Sync),
    agent: &'a A,
    prompt_selector: &'a PromptSelector,
    config: &'a RunnerConfig,
    cancel: &'a CancellationToken,
    stdio_sink: &'a Arc<dyn crate::process::stdio::StdioSink>,
}

type StageError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Pre-iteration failure from a queue `dequeue` call.
///
/// Distinct from [`ProcessingFailure`] because dequeue errors are
/// handled asymmetrically by the run loop: they do **not** bump the
/// iteration counter, do **not** update streak state, and are **not**
/// subject to the `once` policy.
struct DequeueError {
    message: String,
    source: StageError,
}

impl DequeueError {
    fn new<E: std::error::Error + Send + Sync + 'static>(err: E) -> Self {
        let message = err.to_string();
        Self {
            message,
            source: Box::new(err),
        }
    }

    fn message(&self) -> &str {
        &self.message
    }

    fn into_exit_error(
        self,
        event_handler_error_count: u32,
        observer_error_count: u32,
    ) -> RunnerExitError {
        RunnerExitError::DequeueFailed {
            message: self.message,
            source: self.source,
            event_handler_error_count,
            observer_error_count,
        }
    }
}

/// Typed failure produced during signal processing (post-dequeue).
enum ProcessingFailure {
    Render {
        signal_id: SignalId,
        source: StageError,
        message: String,
    },
    Setup {
        signal_id: SignalId,
        source: StageError,
        message: String,
    },
    Agent {
        signal_id: SignalId,
        source: StageError,
        message: String,
        exit: Option<i32>,
    },
    Teardown {
        signal_id: SignalId,
        source: StageError,
        message: String,
    },
}

impl ProcessingFailure {
    fn signal_id(&self) -> SignalId {
        match self {
            Self::Render { signal_id, .. }
            | Self::Setup { signal_id, .. }
            | Self::Agent { signal_id, .. }
            | Self::Teardown { signal_id, .. } => *signal_id,
        }
    }

    fn exit(&self) -> Option<i32> {
        match self {
            Self::Agent { exit, .. } => *exit,
            _ => None,
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Render { message, .. }
            | Self::Setup { message, .. }
            | Self::Agent { message, .. }
            | Self::Teardown { message, .. } => message,
        }
    }

    fn stage_label(&self) -> &'static str {
        match self {
            Self::Render { .. } => "render_prompt",
            Self::Setup { .. } => "workspace_setup",
            Self::Agent { .. } => "agent_run",
            Self::Teardown { .. } => "workspace_teardown",
        }
    }

    fn into_exit_error(
        self,
        event_handler_error_count: u32,
        observer_error_count: u32,
    ) -> RunnerExitError {
        match self {
            Self::Render {
                source, message, ..
            } => RunnerExitError::RenderPromptFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::Setup {
                source, message, ..
            } => RunnerExitError::WorkspaceSetupFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::Agent {
                source, message, ..
            } => RunnerExitError::AgentRunFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::Teardown {
                source, message, ..
            } => RunnerExitError::WorkspaceTeardownFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
        }
    }
}

/// Outcome of one acquisition attempt — flat enum so `run_loop` can match
/// without nested `Option<Result<Option<…>>>`. `next_signal` does NOT emit
/// `RunnerError`; emission lives in `run_loop` so it stays in one place.
enum NextSignal {
    Got(Signal),
    Drained,
    Cancelled,
    Failed(DequeueError),
}

enum FailureDecision {
    Retry,
    Once,
    Bubble,
}

fn decide_after_processing_failure(cfg: &RunnerConfig) -> FailureDecision {
    if !cfg.continue_on_error {
        return FailureDecision::Bubble;
    }
    if cfg.once {
        return FailureDecision::Once;
    }
    FailureDecision::Retry
}

/// Build a `RunnerSummary` with placeholder error counts. The post-loop
/// `runner_finished` emit can itself raise handler errors, so the counts
/// are patched by [`with_counters`] after the emission completes. This
/// mirrors the historical behaviour where the counts in the summary are
/// the final tallies, including handlers fired during `RunnerFinished`.
fn summary(
    reason: RunnerTerminationReason,
    iteration_count: u32,
    last_signal_id: Option<SignalId>,
) -> RunnerSummary {
    RunnerSummary {
        iteration_count,
        last_signal_id,
        termination_reason: reason,
        event_handler_error_count: 0,
        observer_error_count: 0,
    }
}

fn with_counters(
    outcome: Result<RunnerSummary, RunnerExitError>,
    handler_error_count: u32,
    observer_error_count: u32,
) -> Result<RunnerSummary, RunnerExitError> {
    match outcome {
        Ok(mut s) => {
            s.event_handler_error_count = handler_error_count;
            s.observer_error_count = observer_error_count;
            Ok(s)
        }
        Err(err) => Err(err.with_counters(handler_error_count, observer_error_count)),
    }
}

/// Acquire the next signal: park on the queue, race a non-blocking
/// dequeue against synthesise, or synthesise outright depending on
/// `(queue, behavior)`. Pure acquisition — no events, no I/O on the
/// emitter.
async fn next_signal<Q, W, A>(ctx: &RunCtx<'_, Q, W, A>, iteration_count: u32) -> NextSignal
where
    Q: Queue + 'static,
    W: Workspace,
    A: Agent,
{
    match (ctx.queue, &ctx.config.behavior) {
        (Some(queue), RunnerBehavior::Wait) => {
            let dequeued = tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => None,
                res = queue.dequeue(ctx.cancel.clone()) => Some(res),
            };
            match dequeued {
                None => NextSignal::Cancelled,
                Some(Ok(None)) => NextSignal::Drained,
                Some(Ok(Some(signal))) => NextSignal::Got(signal),
                Some(Err(err)) => NextSignal::Failed(DequeueError::new(err)),
            }
        }
        (Some(queue), RunnerBehavior::Loop { delay }) => {
            let dequeued = tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => Ok(None),
                res = queue.dequeue(ctx.cancel.clone()) => res,
                () = tokio::task::yield_now() => Ok(None),
            };
            match dequeued {
                Ok(Some(signal)) => NextSignal::Got(signal),
                Ok(None) => {
                    if ctx.cancel.is_cancelled() {
                        return NextSignal::Cancelled;
                    }
                    if iteration_count > 0 {
                        if let Some(d) = delay {
                            if !d.is_zero() {
                                tokio::select! {
                                    biased;
                                    () = ctx.cancel.cancelled() => {}
                                    () = tokio::time::sleep(*d) => {}
                                }
                            }
                        }
                        if ctx.cancel.is_cancelled() {
                            return NextSignal::Cancelled;
                        }
                    }
                    NextSignal::Got(Signal::synthesized())
                }
                Err(err) => NextSignal::Failed(DequeueError::new(err)),
            }
        }
        (None, RunnerBehavior::Loop { delay }) => {
            if iteration_count > 0 {
                if let Some(d) = delay {
                    if !d.is_zero() {
                        tokio::select! {
                            biased;
                            () = ctx.cancel.cancelled() => {}
                            () = tokio::time::sleep(*d) => {}
                        }
                    }
                }
                if ctx.cancel.is_cancelled() {
                    return NextSignal::Cancelled;
                }
            }
            NextSignal::Got(Signal::synthesized())
        }
        (None, RunnerBehavior::Wait) => {
            unreachable!("(queue=None, behavior=Wait) is rejected at builder time")
        }
    }
}

fn render_prompt(
    selector: &PromptSelector,
    signal: &Signal,
    snap: &IterationContext,
    signal_id: SignalId,
) -> Result<Prompt, ProcessingFailure> {
    selector.render(signal, snap).map_err(|err| {
        let message = err.to_string();
        ProcessingFailure::Render {
            signal_id,
            source: Box::new(err),
            message,
        }
    })
}

/// Successful agent run record — carried out of `drive_workspace` so
/// the caller can finalise iteration state without re-deriving anything
/// from the report.
struct AgentRecord {
    exit_code: Option<i32>,
}

/// Best-effort workspace cleanup after a setup or agent-run failure.
///
/// Calls `Workspace::teardown` once without emitting lifecycle events.
/// If teardown also fails, logs via `tracing::warn!`.
async fn best_effort_teardown<W>(
    workspace: &mut W,
    signal_id: SignalId,
    stage_label: &str,
    cancel: &CancellationToken,
) where
    W: Workspace,
{
    if let Err(teardown_err) = workspace.teardown(cancel.clone()).await {
        let message = teardown_err.to_string();
        let span = tracing::Span::current();
        iter_tracing::record_span_error(&span, "workspace_teardown", &message);
        tracing::warn!(
            signal_id = %signal_id,
            stage = stage_label,
            error = %message,
            "best-effort workspace teardown after stage failure returned an \
             error; workspace may not be fully cleaned up",
        );
    }
}

/// Drive the workspace bracket — setup → agent → teardown — for one
/// signal, emitting lifecycle events at each step.
#[allow(clippy::too_many_lines)]
async fn drive_workspace<Q, W, A>(
    ctx: &RunCtx<'_, Q, W, A>,
    events: &mut RunnerEvents,
    signal: &Signal,
    prompt: &Prompt,
    snap: &IterationContext,
) -> Result<AgentRecord, ProcessingFailure>
where
    Q: Queue + 'static,
    W: Workspace + 'static,
    A: Agent + 'static,
    A::Error: Into<crate::agent::AgentError>,
{
    let signal_id = signal.id();
    let mut workspace = (ctx.workspace_factory)();

    events.workspace_setup_starting(signal, snap).await;

    let setup_span = tracing::info_span!(
        "iter.workspace.setup",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.workspace.type = std::any::type_name::<W>(),
        iter.workspace.path = field::Empty,
    );
    if let Err(err) = workspace
        .setup(ctx.cancel.clone())
        .instrument(setup_span.clone())
        .await
    {
        let message = err.to_string();
        iter_tracing::record_span_error(&setup_span, "workspace_setup", &message);
        events
            .workspace_setup_failed(signal_id, &message, snap)
            .await;
        best_effort_teardown(&mut workspace, signal_id, "workspace_setup", ctx.cancel).await;
        return Err(ProcessingFailure::Setup {
            signal_id,
            source: Box::new(err),
            message,
        });
    }

    let workspace_path = workspace.path().to_path_buf();
    let workspace_path_attr = workspace_path
        .canonicalize()
        .unwrap_or_else(|_| workspace_path.clone());
    setup_span.record(
        "iter.workspace.path",
        field::display(workspace_path_attr.display()),
    );
    events
        .workspace_setup_finished(signal, &workspace_path, snap)
        .await;

    events
        .agent_starting(signal, &workspace_path, prompt, snap)
        .await;

    let agent_ctx =
        crate::agent::AgentRunContext::new(&workspace_path, prompt, ctx.cancel.clone(), signal_id)
            .with_signal_kind(signal.kind())
            .with_stdio_sink(ctx.stdio_sink.clone())
            .with_iteration_timeout(ctx.config.iteration_timeout);
    let agent_span = tracing::info_span!(
        "iter.agent.run",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.agent.type = std::any::type_name::<A>(),
        iter.workspace.path = %workspace_path_attr.display(),
        iter.prompt.bytes = prompt.as_str().len(),
        iter.agent.outcome = field::Empty,
        iter.agent.exit_code = field::Empty,
    );
    let agent_result = crate::agent::run_with_timeout(ctx.agent, agent_ctx)
        .instrument(agent_span.clone())
        .await;

    let outcome_kind = match &agent_result {
        Ok(rep) => AgentOutcomeKind::from_report(rep),
        Err(e) => AgentOutcomeKind::from_error(e),
    };
    let exit_code = agent_result
        .as_ref()
        .ok()
        .and_then(|rep| match rep.exit_status {
            crate::agent::ExitStatus::Success => Some(0),
            crate::agent::ExitStatus::Failure(c) => Some(c),
            crate::agent::ExitStatus::Signal(_) | crate::agent::ExitStatus::Unknown => None,
        });
    agent_span.record("iter.agent.outcome", agent_outcome_label(outcome_kind));
    if let Some(exit_code) = exit_code {
        agent_span.record("iter.agent.exit_code", exit_code);
    }
    if outcome_kind != AgentOutcomeKind::Success {
        iter_tracing::record_span_error(
            &agent_span,
            "agent_run",
            &agent_outcome_message(outcome_kind, exit_code),
        );
    }
    let agent_for_event = agent_result
        .as_ref()
        .map(Clone::clone)
        .map_err(ToString::to_string);

    events
        .agent_finished(
            signal,
            &workspace_path,
            agent_for_event,
            outcome_kind,
            exit_code,
            snap,
        )
        .await;

    if let Err(err) = agent_result {
        let message = err.to_string();
        iter_tracing::record_span_error(&agent_span, "agent_run", &message);
        events.agent_run_failed(signal_id, &message, snap).await;
        best_effort_teardown(&mut workspace, signal_id, "agent_run", ctx.cancel).await;
        return Err(ProcessingFailure::Agent {
            signal_id,
            source: Box::new(err),
            message,
            exit: exit_code,
        });
    }

    events
        .workspace_teardown_starting(signal, &workspace_path, snap)
        .await;

    let teardown_span = tracing::info_span!(
        "iter.workspace.teardown",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.workspace.type = std::any::type_name::<W>(),
        iter.workspace.path = %workspace_path_attr.display(),
    );
    if let Err(err) = workspace
        .teardown(ctx.cancel.clone())
        .instrument(teardown_span.clone())
        .await
    {
        let message = err.to_string();
        iter_tracing::record_span_error(&teardown_span, "workspace_teardown", &message);
        events
            .workspace_teardown_failed(signal_id, &message, snap)
            .await;
        return Err(ProcessingFailure::Teardown {
            signal_id,
            source: Box::new(err),
            message,
        });
    }
    let final_path = workspace.final_path().to_path_buf();
    events
        .workspace_teardown_finished(signal, final_path, snap)
        .await;

    Ok(AgentRecord { exit_code })
}

fn agent_outcome_label(kind: AgentOutcomeKind) -> &'static str {
    match kind {
        AgentOutcomeKind::Success => "success",
        AgentOutcomeKind::Failure => "failure",
        AgentOutcomeKind::TerminatedBySignal => "terminated_by_signal",
        AgentOutcomeKind::UnknownExit => "unknown_exit",
        AgentOutcomeKind::Cancelled => "cancelled",
        AgentOutcomeKind::Errored => "errored",
    }
}

fn agent_outcome_message(kind: AgentOutcomeKind, exit_code: Option<i32>) -> String {
    match exit_code {
        Some(code) => format!(
            "agent outcome {} with exit code {code}",
            agent_outcome_label(kind)
        ),
        None => format!("agent outcome {}", agent_outcome_label(kind)),
    }
}

async fn process_signal<Q, W, A>(
    ctx: &RunCtx<'_, Q, W, A>,
    events: &mut RunnerEvents,
    iter_state: &mut IterationState,
    iteration_count: u32,
    signal: Signal,
) -> Result<(), ProcessingFailure>
where
    Q: Queue + 'static,
    W: Workspace + 'static,
    A: Agent + 'static,
    A::Error: Into<crate::agent::AgentError>,
{
    let now = Utc::now();
    iter_state.begin_iteration(now);
    let snap = iter_state.snapshot(iteration_count + 1);
    let signal_id = signal.id();

    events.signal_received(&signal, now, &snap).await;
    let prompt = render_prompt(ctx.prompt_selector, &signal, &snap, signal_id)?;
    let record = drive_workspace(ctx, events, &signal, &prompt, &snap).await?;
    iter_state.record_success(signal_id, record.exit_code, Utc::now());
    Ok(())
}

/// Drive repetition + termination policy.
///
/// Treats dequeue failures and stage failures **asymmetrically**: dequeue
/// failures do NOT bump `iteration_count` and do NOT call
/// `iter_state.record_failure` — they happen pre-iteration. Only
/// `process_signal` errors bump the counter and update streak state.
#[allow(clippy::too_many_lines)]
async fn run_loop<Q, W, A>(
    ctx: &RunCtx<'_, Q, W, A>,
    events: &mut RunnerEvents,
    iter_state: &mut IterationState,
    iteration_count: &mut u32,
    last_signal_id: &mut Option<SignalId>,
) -> Result<RunnerSummary, RunnerExitError>
where
    Q: Queue + 'static,
    W: Workspace + 'static,
    A: Agent + 'static,
    A::Error: Into<crate::agent::AgentError>,
{
    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(summary(
                RunnerTerminationReason::Cancelled,
                *iteration_count,
                *last_signal_id,
            ));
        }

        // Pre-iteration snapshot (count = iteration_count + 1) so a
        // dequeue-failure `runner_error` hook still sees the turn
        // number that *would* have run.
        let snap = iter_state.snapshot(*iteration_count + 1);

        match next_signal(ctx, *iteration_count).await {
            NextSignal::Drained => {
                return Ok(summary(
                    RunnerTerminationReason::QueueDrained,
                    *iteration_count,
                    *last_signal_id,
                ));
            }
            NextSignal::Cancelled => {
                return Ok(summary(
                    RunnerTerminationReason::Cancelled,
                    *iteration_count,
                    *last_signal_id,
                ));
            }
            NextSignal::Failed(dequeue_err) => {
                events.dequeue_failed(dequeue_err.message(), &snap).await;
                if !ctx.config.continue_on_error {
                    return Err(dequeue_err.into_exit_error(0, 0));
                }
            }
            NextSignal::Got(signal) if signal.is_terminate() => {
                *last_signal_id = Some(signal.id());
                return Ok(summary(
                    RunnerTerminationReason::TerminateSignalReceived,
                    *iteration_count,
                    *last_signal_id,
                ));
            }
            NextSignal::Got(signal) => {
                let signal_id = signal.id();
                *last_signal_id = Some(signal_id);
                let iteration_number = *iteration_count + 1;
                let span = tracing::info_span!(
                    "iter.runner.iteration",
                    iter.signal.id = %signal_id,
                    iter.signal.kind = %signal.kind(),
                    iter.signal.created_at = %signal.created_at().to_rfc3339(),
                    iter.signal.metadata.count = signal.metadata().len(),
                    iter.iteration.count = iteration_number,
                    iter.runner.behavior = ?ctx.config.behavior,
                    iter.runner.once = ctx.config.once,
                    iter.runner.continue_on_error = ctx.config.continue_on_error,
                    iter.runner.iteration_timeout_ms = ?ctx.config.iteration_timeout.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                    iter.runner.outcome = field::Empty,
                );
                iter_tracing::set_span_as_trace_root(&span);
                if let Some(span_context) = crate::telemetry::span_context_from_signal(&signal) {
                    iter_tracing::add_span_link(&span, span_context);
                }
                match process_signal(ctx, events, iter_state, *iteration_count, signal)
                    .instrument(span.clone())
                    .await
                {
                    Ok(()) => {
                        span.record("iter.runner.outcome", "success");
                        *iteration_count += 1;
                        if ctx.config.once {
                            return Ok(summary(
                                RunnerTerminationReason::Once,
                                *iteration_count,
                                *last_signal_id,
                            ));
                        }
                    }
                    Err(failure) => {
                        // drive_workspace failures already emit their error
                        // event; only render_prompt failures reach here
                        // without a prior emission.
                        span.record("iter.runner.outcome", "failure");
                        iter_tracing::record_span_error(
                            &span,
                            failure.stage_label(),
                            failure.message(),
                        );
                        if let ProcessingFailure::Render {
                            signal_id,
                            ref message,
                            ..
                        } = failure
                        {
                            events.render_prompt_failed(signal_id, message, &snap).await;
                        }
                        iter_state.record_failure(failure.signal_id(), failure.exit(), Utc::now());
                        *iteration_count += 1;
                        match decide_after_processing_failure(ctx.config) {
                            FailureDecision::Retry => {}
                            FailureDecision::Once => {
                                return Ok(summary(
                                    RunnerTerminationReason::Once,
                                    *iteration_count,
                                    *last_signal_id,
                                ));
                            }
                            FailureDecision::Bubble => {
                                return Err(failure.into_exit_error(0, 0));
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    //! Exact-once emission tests for the runner-level
    //! `RunnerStarting` / `RunnerFinished` events across every
    //! termination path. These are the load-bearing tests for the
    //! labeled-`'run_loop` design: a regression that adds a `return`
    //! escape hatch (or a new `break 'run_loop` site that bypasses the
    //! post-loop emit) would silently drop one of the events and these
    //! tests would catch it.
    //!
    //! Each test uses a `CapturingHandler` that pushes every received
    //! `Event` into a shared `Vec`, and asserts:
    //!   * `RunnerStarting` appears exactly once,
    //!   * `RunnerFinished` appears exactly once,
    //!   * the `RunnerFinished` `reason` matches the expected
    //!     termination reason.
    //!
    //! The signal-processing stages are stubbed via fake `Workspace`
    //! and `Agent` impls. We rely on `InMemoryQueue` for the queue.
    use std::convert::Infallible;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::agent::{AgentReport, AgentRunContext};
    use crate::prompt::PromptTemplate;
    use crate::queue::{InMemoryQueue, Priority};
    use crate::signal::{Metadata, Signal};

    struct FakeWorkspace {
        path: PathBuf,
    }

    impl Workspace for FakeWorkspace {
        type Error = Infallible;
        async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Ok(())
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn final_path(&self) -> &Path {
            self.path()
        }
    }

    /// Workspace whose `teardown()` always fails. The `teardown_calls`
    /// counter records every invocation so tests can assert that
    /// best-effort cleanup actually ran (not merely that no teardown
    /// event surfaced).
    struct FailingTeardownWorkspace {
        path: PathBuf,
        teardown_calls: Arc<AtomicUsize>,
    }

    impl Workspace for FailingTeardownWorkspace {
        type Error = std::io::Error;
        async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            self.teardown_calls.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("boom"))
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn final_path(&self) -> &Path {
            self.path()
        }
    }

    /// Workspace whose `setup()` AND `teardown()` both fail. Used to
    /// exercise the silent-teardown contract on the setup-failure branch
    /// of `drive_workspace`: when setup fails the runner attempts a
    /// best-effort teardown internally, but a teardown error there must
    /// NOT surface as a second `RunnerError(WorkspaceTeardown)` event.
    /// The `teardown_calls` counter records every invocation so tests
    /// can assert the best-effort cleanup actually ran.
    struct FailingSetupWorkspace {
        path: PathBuf,
        teardown_calls: Arc<AtomicUsize>,
    }

    impl Workspace for FailingSetupWorkspace {
        type Error = std::io::Error;
        async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Err(std::io::Error::other("setup-boom"))
        }
        async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            self.teardown_calls.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("teardown-boom"))
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn final_path(&self) -> &Path {
            self.path()
        }
    }

    /// Workspace that simulates a transient working directory (like
    /// `CloneWorkspace`): `path()` returns the temp dir while active,
    /// `final_path()` always returns the durable base.
    struct TransientWorkspace {
        base: PathBuf,
        temp: tempfile::TempDir,
    }

    impl TransientWorkspace {
        fn new(base: PathBuf) -> Self {
            Self {
                base,
                temp: tempfile::TempDir::new().expect("tempdir"),
            }
        }
    }

    impl Workspace for TransientWorkspace {
        type Error = Infallible;
        async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), Self::Error> {
            Ok(())
        }
        fn path(&self) -> &Path {
            self.temp.path()
        }
        fn final_path(&self) -> &Path {
            &self.base
        }
    }

    #[derive(Default)]
    struct StubAgent;

    impl Agent for StubAgent {
        type Error = crate::agent::AgentError;
        async fn run(&self, _ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
            Ok(AgentReport::success())
        }
    }

    /// Agent that always returns an error. Used to assert the
    /// agent-failure event-ordering contract: `RunnerError(AgentRun)`
    /// fires immediately after `AgentFinished` and `WorkspaceTeardown*`
    /// events are suppressed for the failing iteration.
    struct FailingAgent;

    impl Agent for FailingAgent {
        type Error = crate::agent::AgentError;
        async fn run(&self, _ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
            Err(crate::agent::AgentError::Cancelled)
        }
    }

    /// Agent that sleeps for `delay`, observing `ctx.cancel`. Used to
    /// exercise the per-iteration timeout: the runner cancels its child
    /// token, the agent's `select!` arm fires, and the agent returns
    /// `Cancelled`.
    ///
    /// `cancel_observed` is set when the agent's cancel arm actually ran
    /// (as opposed to the future being dropped from underneath).  Tests
    /// pin a copy and assert it transitioned, which catches regressions
    /// where the runner drops the agent future on `iteration_timeout`
    /// instead of awaiting its graceful shutdown.
    struct SleepyAgent {
        delay: Duration,
        cancel_observed: Arc<AtomicBool>,
    }

    impl SleepyAgent {
        fn new(delay: Duration) -> Self {
            Self {
                delay,
                cancel_observed: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    /// Agent that observes `ctx.cancel` *and then* takes a long time to
    /// actually return — simulating an agent stuck in slow cleanup
    /// (`group.terminate(GRACE)` for a non-responsive child, etc.).
    /// Used to verify the runner does not silently wait out the full
    /// drain grace period when the parent `cancel` fires during the
    /// drain window.
    struct SluggishCleanupAgent {
        post_cancel_sleep: Duration,
    }

    impl Agent for SluggishCleanupAgent {
        type Error = crate::agent::AgentError;
        async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
            ctx.cancel.cancelled().await;
            // Stay alive much longer than DRAIN_GRACE so that any test
            // that bounded its overall wait at < DRAIN_GRACE could only
            // succeed if the runner short-circuited the drain.
            tokio::time::sleep(self.post_cancel_sleep).await;
            Err(crate::agent::AgentError::Cancelled)
        }
    }

    impl Agent for SleepyAgent {
        type Error = crate::agent::AgentError;
        async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
            tokio::select! {
                () = tokio::time::sleep(self.delay) => Ok(AgentReport::success()),
                () = ctx.cancel.cancelled() => {
                    self.cancel_observed.store(true, Ordering::SeqCst);
                    Err(crate::agent::AgentError::Cancelled)
                }
            }
        }
    }

    /// Captures every `Event` the runner emits. Wrapped in `Arc<Mutex>`
    /// so the test can read it back after the runner returns.
    #[derive(Default, Clone)]
    struct CapturingHandler {
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl EventHandler for CapturingHandler {
        async fn handle(
            &self,
            event: &Event,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    /// Like `CapturingHandler` but its first invocation returns `Err`.
    /// Used to verify a failing `RunnerStarting` handler counts into
    /// `event_handler_error_count` and does not abort the run.
    #[derive(Clone)]
    struct FailFirstHandler {
        events: Arc<Mutex<Vec<Event>>>,
        calls: Arc<AtomicUsize>,
    }

    impl EventHandler for FailFirstHandler {
        async fn handle(
            &self,
            event: &Event,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            self.events.lock().unwrap().push(event.clone());
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("first emit fails".into())
            } else {
                Ok(())
            }
        }
    }

    fn make_provider() -> impl Fn() -> FakeWorkspace + Send + Sync {
        || FakeWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
        }
    }

    fn make_failing_teardown_provider() -> (
        impl Fn() -> FailingTeardownWorkspace + Send + Sync,
        Arc<AtomicUsize>,
    ) {
        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&teardown_calls);
        let factory = move || FailingTeardownWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
            teardown_calls: Arc::clone(&counter),
        };
        (factory, teardown_calls)
    }

    fn make_failing_setup_provider() -> (
        impl Fn() -> FailingSetupWorkspace + Send + Sync,
        Arc<AtomicUsize>,
    ) {
        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&teardown_calls);
        let factory = move || FailingSetupWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
            teardown_calls: Arc::clone(&counter),
        };
        (factory, teardown_calls)
    }

    fn count_runner_starting(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, Event::RunnerStarting {}))
            .count()
    }

    fn count_runner_finished(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, Event::RunnerFinished { .. }))
            .count()
    }

    fn finished_reason(events: &[Event]) -> RunnerTerminationReason {
        for e in events {
            if let Event::RunnerFinished { reason, .. } = e {
                return reason.clone();
            }
        }
        panic!("no RunnerFinished in events: {events:?}");
    }

    #[tokio::test]
    async fn once_path_emits_runner_starting_and_finished_exactly_once() {
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary = runner
            .run(CancellationToken::new())
            .await
            .expect("once path returns Ok");
        assert_eq!(summary.iteration_count, 1);
        assert!(matches!(
            summary.termination_reason,
            RunnerTerminationReason::Once
        ));

        let events = handler.events.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1, "starting once");
        assert_eq!(count_runner_finished(&events), 1, "finished once");
        assert!(matches!(
            finished_reason(&events),
            RunnerTerminationReason::Once
        ));
    }

    #[tokio::test]
    async fn cancel_path_emits_runner_starting_and_finished_exactly_once() {
        let queue = Arc::new(InMemoryQueue::new());
        let handler = CapturingHandler::default();
        let cancel = CancellationToken::new();
        // Pre-cancel so the runner short-circuits on the first
        // `cancel.is_cancelled()` check at the top of the loop.
        cancel.cancel();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary = runner.run(cancel).await.expect("cancel returns Ok");
        assert_eq!(summary.iteration_count, 0);
        assert!(matches!(
            summary.termination_reason,
            RunnerTerminationReason::Cancelled
        ));

        let events = handler.events.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
        assert!(matches!(
            finished_reason(&events),
            RunnerTerminationReason::Cancelled
        ));
    }

    #[tokio::test]
    async fn drained_path_emits_runner_starting_and_finished_exactly_once() {
        let queue = Arc::new(InMemoryQueue::new());
        // Close immediately so dequeue returns None and the runner
        // takes the QueueDrained exit branch.
        Queue::close(queue.as_ref()).await.unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary = runner
            .run(CancellationToken::new())
            .await
            .expect("drained returns Ok");
        assert!(matches!(
            summary.termination_reason,
            RunnerTerminationReason::QueueDrained
        ));

        let events = handler.events.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
        assert!(matches!(
            finished_reason(&events),
            RunnerTerminationReason::QueueDrained
        ));
    }

    #[tokio::test]
    async fn stage_error_path_emits_runner_starting_and_finished_exactly_once() {
        // Workspace teardown fails with continue_on_error=false →
        // `RunnerExitError`. The post-loop block must
        // still emit RunnerFinished with an Error reason.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let (provider, _teardown_calls) = make_failing_teardown_provider();
        let runner = Runner::<InMemoryQueue, FailingTeardownWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let err = runner
            .run(CancellationToken::new())
            .await
            .expect_err("teardown failure becomes WorkspaceTeardownFailed");
        assert!(
            matches!(&err, RunnerExitError::WorkspaceTeardownFailed { .. }),
            "expected WorkspaceTeardownFailed, got {err:?}"
        );

        let events = handler.events.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1, "starting once on Err");
        assert_eq!(count_runner_finished(&events), 1, "finished once on Err");
        let reason = finished_reason(&events);
        match reason {
            RunnerTerminationReason::Error { stage, .. } => {
                assert_eq!(stage, ErrorStage::WorkspaceTeardown);
            }
            other => panic!("expected Error reason, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stage_error_path_carries_handler_counts_in_exit_error() {
        // The `Err` exit path now propagates handler/observer counts
        // via error variant fields, mirroring the `Ok` path's
        // `RunnerSummary` fields. This test asserts the count survives.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let events_buf: Arc<Mutex<Vec<Event>>> = Arc::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = FailFirstHandler {
            events: Arc::clone(&events_buf),
            calls: Arc::clone(&calls),
        };
        let (provider, _teardown_calls) = make_failing_teardown_provider();
        let runner = Runner::<InMemoryQueue, FailingTeardownWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler)
            .build()
            .unwrap();

        let err = runner
            .run(CancellationToken::new())
            .await
            .expect_err("teardown failure becomes WorkspaceTeardownFailed");
        match err {
            RunnerExitError::WorkspaceTeardownFailed {
                event_handler_error_count,
                ..
            } => {
                assert!(
                    event_handler_error_count >= 1,
                    "the failing first handler invocation must be counted",
                );
            }
            other => panic!("expected WorkspaceTeardownFailed, got {other:?}"),
        }
    }

    /// Captures `(Event, IterationContext)` pairs for every emit so
    /// tests can assert on the iteration snapshot the runner threaded
    /// through alongside each event. The matching `CapturingHandler`
    /// drops the iteration argument; tests that need to inspect it use
    /// this one.
    #[derive(Default, Clone)]
    struct CapturingIterHandler {
        events: Arc<Mutex<Vec<(Event, IterationContext)>>>,
    }

    impl EventHandler for CapturingIterHandler {
        async fn handle(
            &self,
            event: &Event,
            iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            self.events
                .lock()
                .unwrap()
                .push((event.clone(), iteration.clone()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn teardown_failure_with_continue_on_error_carries_errored_outcome_to_next_iter() {
        // With `continue_on_error = true`, a teardown failure must record
        // a failure on the iteration accumulator before bumping the
        // counter — so the *next* iteration's prompt/template sees
        // `iteration.previous_outcome == "errored"` and
        // `consecutive_failures >= 1`. Regression for the open-coded
        // teardown branch that previously fell through to
        // `record_success`, which mis-marked the failed turn as a win
        // for the next snapshot.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingIterHandler::default();
        let (provider, _teardown_calls) = make_failing_teardown_provider();
        let runner = Runner::<InMemoryQueue, FailingTeardownWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        // Wait blocks once the queue drains; cancel after both signals
        // have been processed.
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let queue_for_task = Arc::clone(&queue);
        let cancel_task = tokio::spawn(async move {
            for _ in 0..200 {
                if queue_for_task.len().await == 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    cancel_for_task.cancel();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            cancel_for_task.cancel();
        });

        let summary = runner
            .run(cancel)
            .await
            .expect("continue_on_error keeps the run going across teardown failure");
        cancel_task.await.unwrap();

        assert!(
            summary.iteration_count >= 2,
            "both signals should have been processed; got {}",
            summary.iteration_count
        );

        let events = handler.events.lock().unwrap().clone();

        // Find the second iteration's `AgentStarting` snapshot. By then
        // turn 1's teardown has failed and `record_failure` must have
        // already run, so the threaded `IterationContext` carries the
        // errored outcome forward.
        let second_agent_starting = events
            .iter()
            .filter(|(e, _)| matches!(e, Event::AgentStarting { .. }))
            .nth(1)
            .map(|(_, iter)| iter.clone())
            .expect("a second AgentStarting must have been emitted");
        assert_eq!(
            second_agent_starting.previous_outcome,
            PreviousOutcome::Errored,
            "the failed teardown of iter 1 must have flipped previous_outcome",
        );
        assert!(
            second_agent_starting.consecutive_failures >= 1,
            "consecutive_failures should reflect the prior failure",
        );
        assert_eq!(
            second_agent_starting.consecutive_successes, 0,
            "the failure must have reset the success streak",
        );
        assert_eq!(
            second_agent_starting.count, 2,
            "the second iteration is 1-indexed as 2",
        );
    }

    #[tokio::test]
    async fn runner_starting_handler_error_does_not_abort_runner() {
        // A handler that errors on `RunnerStarting` must not stop the
        // run. The signal still gets processed; `RunnerFinished`
        // still fires; the `event_handler_error_count` reflects the
        // failure.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let events_buf: Arc<Mutex<Vec<Event>>> = Arc::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = FailFirstHandler {
            events: Arc::clone(&events_buf),
            calls: Arc::clone(&calls),
        };
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler)
            .build()
            .unwrap();

        let summary = runner
            .run(CancellationToken::new())
            .await
            .expect("RunnerStarting handler error must not abort");
        assert_eq!(summary.iteration_count, 1);
        assert!(summary.event_handler_error_count >= 1);
        let events = events_buf.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
    }

    #[tokio::test]
    async fn iteration_timeout_kills_long_running_agent() {
        // The agent would sleep effectively forever. With the timeout in
        // place, the agent module cancels the iteration after
        // `iteration_timeout`, surfaces an `AgentError::IterationTimeout`
        // error, and (because `continue_on_error = false`) terminates with
        // `RunnerError`.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, SleepyAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(SleepyAgent::new(Duration::from_secs(60)))
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: Some(Duration::from_millis(150)),
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let started = std::time::Instant::now();
        let result =
            tokio::time::timeout(Duration::from_secs(5), runner.run(CancellationToken::new()))
                .await
                .expect("runner must return well before the agent's 60s sleep");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "iteration_timeout should bound runtime; took {elapsed:?}",
        );
        // continue_on_error = false → the timeout aborts the runner.
        assert!(result.is_err(), "expected RunnerExitError on aborted run");

        let events = handler.events.lock().unwrap().clone();
        let saw_timeout = events.iter().any(|e| match e {
            Event::AgentRunFailed { error, .. } => error.contains("iteration"),
            _ => false,
        });
        assert!(
            saw_timeout,
            "expected an AgentRunFailed event mentioning the timeout, got: {events:?}",
        );
    }

    #[tokio::test]
    async fn iteration_timeout_does_not_fire_when_agent_returns_quickly() {
        // Sanity: a configured timeout must be invisible to runs that
        // complete promptly.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: Some(Duration::from_secs(60)),
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary = runner
            .run(CancellationToken::new())
            .await
            .expect("fast agent must not trip the timeout");
        assert_eq!(summary.iteration_count, 1);
        assert!(matches!(
            summary.termination_reason,
            RunnerTerminationReason::Once
        ));

        let events = handler.events.lock().unwrap().clone();
        let saw_runner_error = events.iter().any(|e| {
            matches!(
                e,
                Event::DequeueFailed { .. }
                    | Event::RenderPromptFailed { .. }
                    | Event::WorkspaceSetupFailed { .. }
                    | Event::AgentRunFailed { .. }
                    | Event::WorkspaceTeardownFailed { .. }
            )
        });
        assert!(
            !saw_runner_error,
            "no runner errors expected, got: {events:?}"
        );
    }

    #[tokio::test]
    async fn iteration_timeout_with_continue_on_error_advances_to_next_iter() {
        // With `continue_on_error = true`, a timed-out iteration becomes
        // a recorded failure and the loop moves on. We use `once = true`
        // so we get a single iteration and observe its outcome via the
        // emitted `AgentFinished` event (`outcome = Errored`).
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, SleepyAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(SleepyAgent::new(Duration::from_secs(60)))
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: Some(Duration::from_millis(150)),
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary =
            tokio::time::timeout(Duration::from_secs(5), runner.run(CancellationToken::new()))
                .await
                .expect("runner returned before deadline")
                .expect("continue_on_error should make a timed-out iter recoverable");
        assert_eq!(summary.iteration_count, 1);

        let events = handler.events.lock().unwrap().clone();
        let saw_agent_run_failed = events
            .iter()
            .any(|e| matches!(e, Event::AgentRunFailed { .. }));
        assert!(
            saw_agent_run_failed,
            "the timed-out iteration must surface as an AgentRunFailed event",
        );
    }

    #[tokio::test]
    async fn iteration_timeout_lets_agent_observe_cancel_before_returning() {
        // Regression: an earlier implementation wrapped the agent future in
        // `tokio::time::timeout` and dropped it on expiry, which fired
        // `ProcessGroup`'s `Drop` synchronously (immediate `SIGKILL`,
        // bypassing `terminate(GRACE)`'s graceful `SIGTERM` → grace →
        // `SIGKILL`).  The runner must instead keep awaiting the agent
        // future after firing the iter-scoped cancel so the agent's own
        // cleanup can run.  We verify the agent's cancel arm actually
        // executed by reading the `cancel_observed` flag after `run`
        // returns.
        let agent = SleepyAgent::new(Duration::from_secs(60));
        let observed = Arc::clone(&agent.cancel_observed);
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, SleepyAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(agent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: Some(Duration::from_millis(100)),
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        drop(
            tokio::time::timeout(Duration::from_secs(5), runner.run(CancellationToken::new()))
                .await
                .expect("runner returned before deadline"),
        );

        assert!(
            observed.load(Ordering::SeqCst),
            "agent's cancel arm must run on iteration_timeout — runner must \
             not drop the agent future on the floor",
        );
    }

    #[tokio::test]
    async fn iteration_timeout_drain_yields_to_parent_cancel() {
        // Regression: the drain that follows `iteration_timeout` (waiting
        // for the agent's graceful shutdown) used to be a bare
        // `tokio::time::timeout(DRAIN_GRACE, ...)` with no arm watching
        // the parent `cancel` token.  An operator Ctrl-C during that
        // window was silently ignored for up to DRAIN_GRACE seconds.
        // The drain now `select!`s the parent token, so this test must
        // complete in well under DRAIN_GRACE.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, SluggishCleanupAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            // Outlast DRAIN_GRACE comfortably so the only way the test
            // returns quickly is via the parent-cancel arm.
            .agent(SluggishCleanupAgent {
                post_cancel_sleep: Duration::from_secs(60),
            })
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: Some(Duration::from_millis(50)),
            })
            .on_all(CapturingHandler::default())
            .build()
            .unwrap();

        let parent_cancel = CancellationToken::new();
        let canceller = parent_cancel.clone();
        // Fire parent cancel after the iteration timeout has fired and
        // the drain has begun, but well before DRAIN_GRACE elapses.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            canceller.cancel();
        });

        let started = std::time::Instant::now();
        drop(
            tokio::time::timeout(Duration::from_secs(5), runner.run(parent_cancel))
                .await
                .expect("runner must short-circuit drain on parent cancel"),
        );
        let elapsed = started.elapsed();

        // DRAIN_GRACE is GRACE + 5s = 10s.  If the drain ignored the
        // parent cancel, the test would not return until ~150ms (cancel
        // fired) + ~10s (full drain) ≈ 10s+.  We bound at 2s.
        assert!(
            elapsed < Duration::from_secs(2),
            "drain must yield to parent cancel; took {elapsed:?}",
        );
    }

    #[tokio::test]
    async fn agent_failure_emits_runner_error_before_teardown_events() {
        // Documented contract (`docs/config/iterfile/on.md`):
        //   `runner_error` "fires instead of any later lifecycle events
        //    for that iteration."
        //
        // Therefore on agent failure the visible event sequence must be:
        //     AgentFinished(Errored) → RunnerError(AgentRun)
        // and `WorkspaceTeardownStarting` / `WorkspaceTeardownFinished`
        // must NOT fire for that iteration. Teardown still runs
        // internally (best-effort) so the workspace is released, but no
        // teardown events surface to handlers.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, FailingAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(FailingAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        runner
            .run(CancellationToken::new())
            .await
            .expect("continue_on_error keeps the run alive across an agent failure");

        let events = handler.events.lock().unwrap().clone();

        let agent_finished_idx = events
            .iter()
            .position(|e| matches!(e, Event::AgentFinished { .. }))
            .expect("AgentFinished must fire even when the agent errored");
        let runner_error_idx = events
            .iter()
            .position(|e| matches!(e, Event::AgentRunFailed { .. }))
            .expect("AgentRunFailed must fire after a failing agent");
        assert!(
            runner_error_idx > agent_finished_idx,
            "AgentRunFailed must follow AgentFinished, got events: {events:?}",
        );

        let runner_error_count = events
            .iter()
            .filter(|e| matches!(e, Event::AgentRunFailed { .. }))
            .count();
        assert_eq!(
            runner_error_count, 1,
            "AgentRunFailed must fire exactly once per failed iteration, got: {events:?}",
        );

        let saw_teardown_event = events.iter().any(|e| {
            matches!(
                e,
                Event::WorkspaceTeardownStarting { .. } | Event::WorkspaceTeardownFinished { .. }
            )
        });
        assert!(
            !saw_teardown_event,
            "Workspace teardown events must be suppressed on agent failure, got: {events:?}",
        );
    }

    #[tokio::test]
    async fn setup_failure_emits_single_runner_error_with_no_teardown_events() {
        // Setup-failure contract (`docs/config/iterfile/on.md`):
        //   `runner_error` "fires instead of any later lifecycle events
        //    for that iteration."
        //
        // When `Workspace::setup` fails, the runner:
        //   1. Emits `RunnerError(WorkspaceSetup)` exactly once.
        //   2. Best-effort tears down internally to release the
        //      workspace, but does NOT emit
        //      `WorkspaceTeardownStarting` / `WorkspaceTeardownFinished`
        //      (per the documented "fires instead of any later lifecycle
        //      events" rule).
        //   3. If that best-effort teardown itself errors, it logs via
        //      `tracing::warn!` but does NOT emit a second
        //      `RunnerError(WorkspaceTeardown)` — the operator already
        //      has the original setup error, and a follow-on teardown
        //      error during the cleanup attempt is noise that would
        //      mislead error-routing handlers.
        //
        // `FailingSetupWorkspace` fails BOTH setup and teardown, so this
        // test exercises every branch of the setup-failure error path.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let (provider, teardown_calls) = make_failing_setup_provider();
        let runner = Runner::<InMemoryQueue, FailingSetupWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        runner
            .run(CancellationToken::new())
            .await
            .expect("continue_on_error keeps the run alive across a setup failure");

        let events = handler.events.lock().unwrap().clone();

        let setup_error_count = events
            .iter()
            .filter(|e| matches!(e, Event::WorkspaceSetupFailed { .. }))
            .count();
        assert_eq!(
            setup_error_count, 1,
            "WorkspaceSetupFailed must fire exactly once on setup failure, got: {events:?}",
        );

        let teardown_error_count = events
            .iter()
            .filter(|e| matches!(e, Event::WorkspaceTeardownFailed { .. }))
            .count();
        assert_eq!(
            teardown_error_count, 0,
            "WorkspaceTeardownFailed must NOT fire when teardown is invoked as best-effort \
             cleanup after a setup failure (silent-teardown contract), got: {events:?}",
        );

        let saw_teardown_event = events.iter().any(|e| {
            matches!(
                e,
                Event::WorkspaceTeardownStarting { .. } | Event::WorkspaceTeardownFinished { .. }
            )
        });
        assert!(
            !saw_teardown_event,
            "Workspace teardown events must be suppressed on setup failure, got: {events:?}",
        );

        let saw_setup_finished = events
            .iter()
            .any(|e| matches!(e, Event::WorkspaceSetupFinished { .. }));
        assert!(
            !saw_setup_finished,
            "WorkspaceSetupFinished must NOT fire when setup itself failed, got: {events:?}",
        );

        // Best-effort cleanup actually ran. Without this assertion the
        // test would still pass if the silent teardown call were
        // deleted entirely from `drive_workspace`.
        assert_eq!(
            teardown_calls.load(Ordering::SeqCst),
            1,
            "Workspace::teardown must be invoked exactly once as best-effort cleanup",
        );
    }

    #[tokio::test]
    async fn agent_failure_with_failing_teardown_emits_single_runner_error() {
        // Pairs with `agent_failure_emits_runner_error_before_teardown_events`
        // (which uses a workspace whose teardown succeeds). This test
        // exercises the OTHER agent-failure branch: the silent-teardown
        // path where best-effort cleanup also fails. The contract is the
        // same as the setup-failure case — exactly one
        // `RunnerError(AgentRun)`, no `RunnerError(WorkspaceTeardown)`,
        // no teardown lifecycle events, and the cleanup attempt actually
        // ran.
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let (provider, teardown_calls) = make_failing_teardown_provider();
        let runner = Runner::<InMemoryQueue, FailingTeardownWorkspace, FailingAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(FailingAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        runner
            .run(CancellationToken::new())
            .await
            .expect("continue_on_error keeps the run alive across an agent failure");

        let events = handler.events.lock().unwrap().clone();

        let agent_error_count = events
            .iter()
            .filter(|e| matches!(e, Event::AgentRunFailed { .. }))
            .count();
        assert_eq!(
            agent_error_count, 1,
            "AgentRunFailed must fire exactly once on agent failure, got: {events:?}",
        );

        let teardown_error_count = events
            .iter()
            .filter(|e| matches!(e, Event::WorkspaceTeardownFailed { .. }))
            .count();
        assert_eq!(
            teardown_error_count, 0,
            "WorkspaceTeardownFailed must NOT fire when teardown is invoked as best-effort \
             cleanup after an agent failure (silent-teardown contract), got: {events:?}",
        );

        let saw_teardown_event = events.iter().any(|e| {
            matches!(
                e,
                Event::WorkspaceTeardownStarting { .. } | Event::WorkspaceTeardownFinished { .. }
            )
        });
        assert!(
            !saw_teardown_event,
            "Workspace teardown events must be suppressed on agent failure, got: {events:?}",
        );

        assert_eq!(
            teardown_calls.load(Ordering::SeqCst),
            1,
            "Workspace::teardown must be invoked exactly once as best-effort cleanup",
        );
    }

    struct FailingQueue {
        fail_count: AtomicUsize,
        max_failures: usize,
        inner: InMemoryQueue,
    }

    impl FailingQueue {
        fn new(max_failures: usize) -> Self {
            Self {
                fail_count: AtomicUsize::new(0),
                max_failures,
                inner: InMemoryQueue::new(),
            }
        }
    }

    impl Queue for FailingQueue {
        type Error = std::io::Error;

        async fn queue(&self, signal: Signal, priority: Priority) -> Result<(), Self::Error> {
            self.inner
                .queue(signal, priority)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))
        }

        async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
            let n = self.fail_count.fetch_add(1, Ordering::SeqCst);
            if n < self.max_failures {
                return Err(std::io::Error::other("dequeue-boom"));
            }
            self.inner
                .dequeue(cancel)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))
        }
    }

    #[tokio::test]
    async fn dequeue_failure_without_continue_on_error_exits_with_stage_dequeue() {
        let queue = Arc::new(FailingQueue::new(1));
        let handler = CapturingHandler::default();
        let runner = Runner::<FailingQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: false,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let err = runner
            .run(CancellationToken::new())
            .await
            .expect_err("dequeue failure without continue_on_error must exit with Err");

        assert!(
            matches!(err, RunnerExitError::DequeueFailed { .. }),
            "expected DequeueFailed, got {err:?}"
        );

        let events = handler.events.lock().unwrap().clone();
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
        match finished_reason(&events) {
            RunnerTerminationReason::Error { stage, .. } => {
                assert_eq!(stage, ErrorStage::Dequeue);
            }
            other => panic!("expected Error reason, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dequeue_failure_with_continue_on_error_retries_and_does_not_bump_iteration() {
        let queue = Arc::new(FailingQueue::new(1));
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<FailingQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: true,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        let summary = runner
            .run(CancellationToken::new())
            .await
            .expect("continue_on_error retries past dequeue failure");

        assert_eq!(
            summary.iteration_count, 1,
            "dequeue failure must not count as an iteration"
        );
        assert!(matches!(
            summary.termination_reason,
            RunnerTerminationReason::Once
        ));

        let events = handler.events.lock().unwrap().clone();
        let dequeue_errors = events
            .iter()
            .filter(|e| matches!(e, Event::DequeueFailed { .. }))
            .count();
        assert_eq!(
            dequeue_errors, 1,
            "DequeueFailed should fire for the failed dequeue"
        );
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
    }

    #[tokio::test]
    async fn terminate_signal_stops_runner_gracefully() {
        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::terminate(), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig::default())
            .on_all(handler.clone())
            .build()
            .unwrap();
        let summary = runner.run(CancellationToken::new()).await.unwrap();
        assert_eq!(
            summary.termination_reason,
            RunnerTerminationReason::TerminateSignalReceived,
        );
        assert_eq!(summary.iteration_count, 0);
        let events = handler.events.lock().unwrap();
        assert_eq!(count_runner_starting(&events), 1);
        assert_eq!(count_runner_finished(&events), 1);
        assert_eq!(
            finished_reason(&events),
            RunnerTerminationReason::TerminateSignalReceived,
        );
    }

    #[tokio::test]
    async fn work_signals_before_terminate_are_all_processed() {
        let queue = Arc::new(InMemoryQueue::new());
        for _ in 0..3 {
            queue
                .queue(Signal::new(Metadata::new()), Priority::default())
                .await
                .unwrap();
        }
        queue
            .queue(Signal::terminate(), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, FakeWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig::default())
            .on_all(handler.clone())
            .build()
            .unwrap();
        let summary = runner.run(CancellationToken::new()).await.unwrap();
        assert_eq!(
            summary.termination_reason,
            RunnerTerminationReason::TerminateSignalReceived,
        );
        assert_eq!(summary.iteration_count, 3);
    }

    #[tokio::test]
    async fn transient_workspace_teardown_event_carries_persistent_path() {
        let persistent_dir = tempfile::TempDir::new().expect("persistent dir");
        let persistent_path = persistent_dir.path().to_path_buf();

        let expected = persistent_path.clone();
        let provider = move || TransientWorkspace::new(persistent_path.clone());

        let queue = Arc::new(InMemoryQueue::new());
        queue
            .queue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
        let handler = CapturingHandler::default();
        let runner = Runner::<InMemoryQueue, TransientWorkspace, StubAgent>::builder()
            .queue(Arc::clone(&queue))
            .workspaces(provider)
            .agent(StubAgent)
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerConfig {
                once: true,
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout: None,
            })
            .on_all(handler.clone())
            .build()
            .unwrap();

        runner
            .run(CancellationToken::new())
            .await
            .expect("run ok");

        let events = handler.events.lock().unwrap().clone();
        let teardown_path = events.iter().find_map(|e| match e {
            Event::WorkspaceTeardownFinished { path, .. } => Some(path.clone()),
            _ => None,
        });
        assert_eq!(
            teardown_path.as_deref(),
            Some(expected.as_path()),
            "post-teardown event must carry the persistent path, not the transient working directory",
        );
    }
}
