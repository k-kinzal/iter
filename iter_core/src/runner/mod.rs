//! [`Runner`] — receives a signal, sets up a workspace, runs an agent, and
//! tears down the workspace.
//!
//! The runner exposes a single `run` method that returns once one of the
//! configured termination conditions fires (or the supplied
//! [`CancellationToken`] is triggered).
//!
//! # Cancellation
//!
//! The Runner is one party in the crate-wide cancellation discipline
//! documented at [`process::interrupt`](crate::process::interrupt). It may
//! *fire* cancellation only through its own iteration timeout
//! (`iteration_timeout`); on *receipt* it owes exactly one thing — complete
//! the current iteration's teardown and report the outcome. It never closes a
//! Queue, kills an Agent's process tree directly, or finalizes a run record;
//! each of those belongs to the party that owns it.

pub mod builder;
pub mod config;
/// Error types for [`Runner::run`].
pub mod error;
pub mod event;
pub mod event_emitter;
pub mod event_handler;
mod events;
pub mod iteration;
pub mod lifecycle;
pub mod observer;
pub mod shell_event_handler;

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, field};

use crate::agent::Agent;
use crate::prompt::{Prompt, PromptSelector};
use crate::queue::Queue;
use crate::signal::{Signal, SignalId};
use crate::workspace::Workspace;

pub use builder::{BuilderError, RunnerBuilder};
pub use config::{RunnerBehavior, RunnerConfig, RunnerSummary, RunnerTerminationReason};
pub use error::RunnerExitError;
pub use event::{Event, EventName};
pub use event_emitter::{EmitReport, EventEmitter};
pub use event_handler::{BoxError, EventHandler};
pub use iteration::{IterationContext, IterationState, PreviousResult};
pub use lifecycle::{RedactedMetadata, RunnerLifecycle};
pub use observer::{DynRunnerObserver, ObserveFuture, RunnerObserver};
pub use shell_event_handler::ShellEventHandler;

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
pub struct Runner<A: Agent> {
    pub(crate) queue: Option<Arc<dyn Queue>>,
    pub(crate) workspaces: Arc<dyn Fn() -> Box<dyn Workspace> + Send + Sync>,
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
    /// [`crate::log::NoopSink`].
    pub(crate) stdio_sink: Arc<dyn crate::log::OutputSink>,
}

impl<A> Runner<A>
where
    A: Agent + 'static,
{
    /// Start a fluent [`RunnerBuilder`].
    pub fn builder() -> RunnerBuilder<A> {
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
    /// * a processing error occurs and `continue_on_error` is `false`.
    ///
    /// When `behavior = loop` is configured the runner synthesises a
    /// signal each time the queue is empty (or whenever it has no queue),
    /// applying the configured `delay` between successive synthesised
    /// iterations. The first iteration runs without delay so a one-shot
    /// `behavior = loop` invocation starts immediately.
    pub async fn run(self, cancel: CancellationToken) -> Result<RunnerSummary, RunnerExitError> {
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
        let mut events = RunnerEvents::new(emitter, observers);
        let runner_started_at = Utc::now();
        let mut iter_state = IterationState::new(runner_started_at);
        let mut iteration_count: u32 = 0;
        let mut last_signal_id: Option<SignalId> = None;

        events.bootstrap(runner_started_at).await;
        let bootstrap_snapshot = iter_state.snapshot(0);
        events.runner_starting(&bootstrap_snapshot).await;

        let loop_result = run_loop(
            queue.as_deref(),
            &*workspace_factory,
            &agent,
            &prompt_selector,
            &config,
            &cancel,
            &stdio_sink,
            &mut events,
            &mut iter_state,
            &mut iteration_count,
            &mut last_signal_id,
        )
        .await;

        let (final_reason, final_iter_count) = match &loop_result {
            Ok(s) => (s.termination_reason.clone(), s.iteration_count),
            Err(err) => (
                RunnerTerminationReason::Error {
                    error_source: err.error_source().to_owned(),
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
            loop_result,
            events.handler_error_count,
            events.observer_error_count,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Composition primitives for `Runner::run`.
//
// Each concern below holds exactly one responsibility: `RunnerEvents`
// (in events.rs) owns the broadcast + tally pair; `ProcessingFailure`
// and `NextSignal` are the data shapes that compose processing results;
// `decide_after_processing_failure` is the pure failure-policy decision;
// the `next_signal` / `render_prompt` / `drive_workspace` functions are
// typed and side-effect-explicit.  Each function receives only the
// parameters it actually uses.
// ─────────────────────────────────────────────────────────────────────────

type BoxedError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Pre-iteration failure from a queue `dequeue` call.
///
/// Distinct from [`ProcessingFailure`] because dequeue errors are
/// handled asymmetrically by the run loop: they do **not** bump the
/// iteration counter, do **not** update streak state, and are **not**
/// subject to the `once` policy.
struct DequeueError {
    message: String,
    source: BoxedError,
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
        source: BoxedError,
        message: String,
    },
    Setup {
        signal_id: SignalId,
        source: BoxedError,
        message: String,
    },
    Agent {
        signal_id: SignalId,
        source: BoxedError,
        message: String,
        exit: Option<i32>,
    },
    Teardown {
        signal_id: SignalId,
        source: BoxedError,
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

    fn error_source(&self) -> &'static str {
        match self {
            Self::Render { .. } => error::error_source::RENDER_PROMPT,
            Self::Setup { .. } => error::error_source::WORKSPACE_SETUP,
            Self::Agent { .. } => error::error_source::AGENT_RUN,
            Self::Teardown { .. } => error::error_source::WORKSPACE_TEARDOWN,
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

/// Result of one acquisition attempt — flat enum so `run_loop` can match
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

/// Decide what to do after a processing failure.
///
/// Since a non-zero / signalled agent run is now an `Err` (an
/// [`AgentError`](crate::agent::AgentError), not an `Ok` carrying a failed
/// exit), this policy governs those runs too: the failing iteration has
/// already been through best-effort workspace teardown (artifacts may
/// exist) and recorded as `previous_result = "errored"`. A non-zero exit is
/// a **non-retryable** failure in the sense that the runner does not re-run
/// the *same* signal — `continue_on_error` only decides whether the loop
/// proceeds to the next signal (`Retry`) or bubbles the error out
/// (`Bubble`); `once` short-circuits to a single iteration.
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
    result: Result<RunnerSummary, RunnerExitError>,
    handler_error_count: u32,
    observer_error_count: u32,
) -> Result<RunnerSummary, RunnerExitError> {
    match result {
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
async fn next_signal(
    queue: Option<&dyn Queue>,
    behavior: &RunnerBehavior,
    cancel: &CancellationToken,
    iteration_count: u32,
) -> NextSignal {
    match (queue, behavior) {
        (Some(queue), RunnerBehavior::Wait) => {
            let dequeued = tokio::select! {
                biased;
                () = cancel.cancelled() => None,
                res = queue.dequeue(cancel.clone()) => Some(res),
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
                () = cancel.cancelled() => Ok(None),
                res = queue.dequeue(cancel.clone()) => res,
                () = tokio::task::yield_now() => Ok(None),
            };
            match dequeued {
                Ok(Some(signal)) => NextSignal::Got(signal),
                Ok(None) => {
                    if cancel.is_cancelled() {
                        return NextSignal::Cancelled;
                    }
                    if iteration_count > 0 {
                        if let Some(d) = delay {
                            if !d.is_zero() {
                                tokio::select! {
                                    biased;
                                    () = cancel.cancelled() => {}
                                    () = tokio::time::sleep(*d) => {}
                                }
                            }
                        }
                        if cancel.is_cancelled() {
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
                            () = cancel.cancelled() => {}
                            () = tokio::time::sleep(*d) => {}
                        }
                    }
                }
                if cancel.is_cancelled() {
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
async fn best_effort_teardown(
    workspace: &mut Box<dyn Workspace>,
    signal_id: SignalId,
    failed_step: &str,
    cancel: &CancellationToken,
) {
    if let Err(teardown_err) = workspace.teardown(cancel.clone()).await {
        let message = teardown_err.to_string();
        let span = tracing::Span::current();
        iter_tracing::record_span_error(&span, "workspace_teardown", &message);
        tracing::warn!(
            signal_id = %signal_id,
            failed_step = failed_step,
            error = %message,
            "best-effort workspace teardown after failure returned an \
             error; workspace may not be fully cleaned up",
        );
    }
}

/// Drive the workspace bracket — setup -> agent -> teardown — for one
/// signal, emitting lifecycle events at each step.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
async fn drive_workspace<A>(
    workspace_factory: &(dyn Fn() -> Box<dyn Workspace> + Send + Sync),
    agent: &A,
    config: &RunnerConfig,
    cancel: &CancellationToken,
    stdio_sink: &Arc<dyn crate::log::OutputSink>,
    events: &mut RunnerEvents,
    signal: &Signal,
    prompt: &Prompt,
    snap: &IterationContext,
) -> Result<AgentRecord, ProcessingFailure>
where
    A: Agent + 'static,
{
    let signal_id = signal.id();
    let mut workspace = (workspace_factory)();
    let workspace_name = workspace.name();

    events.workspace_setup_starting(signal, snap).await;

    let setup_span = tracing::info_span!(
        "iter.workspace.setup",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.workspace.name = workspace_name,
        iter.workspace.path = field::Empty,
    );
    if let Err(err) = workspace
        .setup(cancel.clone())
        .instrument(setup_span.clone())
        .await
    {
        let message = err.to_string();
        iter_tracing::record_span_error(&setup_span, "workspace_setup", &message);
        events
            .runner_error(
                error::error_source::WORKSPACE_SETUP,
                Some(signal_id),
                &message,
                Event::WorkspaceSetupFailed {
                    signal_id,
                    error: message.clone(),
                },
                snap,
            )
            .await;
        best_effort_teardown(&mut workspace, signal_id, "workspace_setup", cancel).await;
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
        crate::agent::AgentRunContext::new(&workspace_path, prompt, cancel.clone(), signal_id)
            .with_signal_kind(signal.kind())
            .with_stdio_sink(stdio_sink.clone())
            .with_iteration_timeout(config.iteration_timeout);
    let agent_span = tracing::info_span!(
        "iter.agent.run",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.agent.r#type = std::any::type_name::<A>(),
        iter.workspace.path = %workspace_path_attr.display(),
        iter.prompt.bytes = prompt.as_str().len(),
        iter.agent.result = field::Empty,
        iter.agent.exit_code = field::Empty,
    );
    let agent_result = crate::agent::run_with_timeout(agent, agent_ctx)
        .instrument(agent_span.clone())
        .await;

    // The agent result is now a plain `Result`: `Ok` means the agent ran a
    // turn (exit 0), `Err` carries the failure class. The lifecycle label and
    // the optional exit code are derived directly from it — there is no
    // separate `result_kind` projection type anymore.
    let (result_label, exit_code): (&'static str, Option<i32>) = match &agent_result {
        Ok(_) => ("success", Some(0)),
        Err(err) => (err.label(), err.exit_code()),
    };
    agent_span.record("iter.agent.result", result_label);
    if let Some(exit_code) = exit_code {
        agent_span.record("iter.agent.exit_code", exit_code);
    }
    if agent_result.is_err() {
        iter_tracing::record_span_error(
            &agent_span,
            "agent_run",
            &agent_result_message(result_label, exit_code),
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
            result_label,
            exit_code,
            snap,
        )
        .await;

    if let Err(err) = agent_result {
        let message = err.to_string();
        iter_tracing::record_span_error(&agent_span, "agent_run", &message);
        events
            .runner_error(
                error::error_source::AGENT_RUN,
                Some(signal_id),
                &message,
                Event::AgentRunFailed {
                    signal_id,
                    error: message.clone(),
                },
                snap,
            )
            .await;
        best_effort_teardown(&mut workspace, signal_id, "agent_run", cancel).await;
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
        iter.workspace.name = workspace_name,
        iter.workspace.path = %workspace_path_attr.display(),
    );
    if let Err(err) = workspace
        .teardown(cancel.clone())
        .instrument(teardown_span.clone())
        .await
    {
        let message = err.to_string();
        iter_tracing::record_span_error(&teardown_span, "workspace_teardown", &message);
        events
            .runner_error(
                error::error_source::WORKSPACE_TEARDOWN,
                Some(signal_id),
                &message,
                Event::WorkspaceTeardownFailed {
                    signal_id,
                    error: message.clone(),
                },
                snap,
            )
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

fn agent_result_message(label: &str, exit_code: Option<i32>) -> String {
    match exit_code {
        Some(code) => format!("agent result {label} with exit code {code}"),
        None => format!("agent result {label}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_signal<A>(
    workspace_factory: &(dyn Fn() -> Box<dyn Workspace> + Send + Sync),
    agent: &A,
    prompt_selector: &PromptSelector,
    config: &RunnerConfig,
    cancel: &CancellationToken,
    stdio_sink: &Arc<dyn crate::log::OutputSink>,
    events: &mut RunnerEvents,
    iter_state: &mut IterationState,
    iteration_count: u32,
    signal: Signal,
) -> Result<(), ProcessingFailure>
where
    A: Agent + 'static,
{
    let now = Utc::now();
    iter_state.begin_iteration(now);
    let snap = iter_state.snapshot(iteration_count + 1);
    let signal_id = signal.id();

    events.signal_received(&signal, now, &snap).await;
    let prompt = match render_prompt(prompt_selector, &signal, &snap, signal_id) {
        Ok(p) => p,
        Err(failure) => {
            if let ProcessingFailure::Render {
                signal_id,
                ref message,
                ..
            } = failure
            {
                events
                    .runner_error(
                        error::error_source::RENDER_PROMPT,
                        Some(signal_id),
                        message,
                        Event::RenderPromptFailed {
                            signal_id,
                            error: message.clone(),
                        },
                        &snap,
                    )
                    .await;
            }
            return Err(failure);
        }
    };
    let record = drive_workspace(
        workspace_factory,
        agent,
        config,
        cancel,
        stdio_sink,
        events,
        &signal,
        &prompt,
        &snap,
    )
    .await?;
    iter_state.record_success(signal_id, record.exit_code, Utc::now());
    Ok(())
}

/// Drive repetition + termination policy.
///
/// Treats dequeue failures and processing failures **asymmetrically**:
/// dequeue failures do NOT bump `iteration_count` and do NOT call
/// `iter_state.record_failure` — they happen pre-iteration. Only
/// `process_signal` errors bump the counter and update streak state.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
async fn run_loop<A>(
    queue: Option<&dyn Queue>,
    workspace_factory: &(dyn Fn() -> Box<dyn Workspace> + Send + Sync),
    agent: &A,
    prompt_selector: &PromptSelector,
    config: &RunnerConfig,
    cancel: &CancellationToken,
    stdio_sink: &Arc<dyn crate::log::OutputSink>,
    events: &mut RunnerEvents,
    iter_state: &mut IterationState,
    iteration_count: &mut u32,
    last_signal_id: &mut Option<SignalId>,
) -> Result<RunnerSummary, RunnerExitError>
where
    A: Agent + 'static,
{
    loop {
        if cancel.is_cancelled() {
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

        match next_signal(queue, &config.behavior, cancel, *iteration_count).await {
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
                events
                    .runner_error(
                        error::error_source::DEQUEUE,
                        None,
                        dequeue_err.message(),
                        Event::DequeueFailed {
                            error: dequeue_err.message().to_owned(),
                        },
                        &snap,
                    )
                    .await;
                if !config.continue_on_error {
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
                    iter.runner.behavior = ?config.behavior,
                    iter.runner.once = config.once,
                    iter.runner.continue_on_error = config.continue_on_error,
                    iter.runner.iteration_timeout_ms = ?config.iteration_timeout.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                    iter.runner.result = field::Empty,
                );
                iter_tracing::set_span_as_trace_root(&span);
                if let Some(span_context) = crate::telemetry::span_context_from_signal(&signal) {
                    iter_tracing::add_span_link(&span, span_context);
                }
                match process_signal(
                    workspace_factory,
                    agent,
                    prompt_selector,
                    config,
                    cancel,
                    stdio_sink,
                    events,
                    iter_state,
                    *iteration_count,
                    signal,
                )
                .instrument(span.clone())
                .await
                {
                    Ok(()) => {
                        span.record("iter.runner.result", "success");
                        *iteration_count += 1;
                        if config.once {
                            return Ok(summary(
                                RunnerTerminationReason::Once,
                                *iteration_count,
                                *last_signal_id,
                            ));
                        }
                    }
                    Err(failure) => {
                        span.record("iter.runner.result", "failure");
                        iter_tracing::record_span_error(
                            &span,
                            failure.error_source(),
                            failure.message(),
                        );
                        iter_state.record_failure(failure.signal_id(), failure.exit(), Utc::now());
                        *iteration_count += 1;
                        match decide_after_processing_failure(config) {
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
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;
