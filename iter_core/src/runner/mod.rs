//! [`Runner`] — receives a signal, sets up a workspace, runs an agent, and
//! tears down the workspace.
//!
//! The runner exposes a single `run` method that returns once one of the
//! configured termination conditions fires (or the supplied
//! [`CancellationToken`] is triggered).
//!
//! # Cancellation
//!
//! The Runner is one party in the crate-wide cancellation discipline. OS
//! interrupts are translated into cancellation by [`crate::os_signal`]; the
//! Runner may *fire* cancellation only through its own iteration timeout
//! (`iteration_timeout`); on *receipt* it owes exactly one thing — complete
//! the current iteration's teardown and report the outcome. It never closes a
//! Queue, kills an Agent's process tree directly, or finalizes a run record;
//! each of those belongs to the party that owns it.

pub mod builder;
pub mod completion;
/// Error types for [`Runner::run`].
pub mod error;
pub mod event;
pub mod event_emitter;
pub mod event_handler;
mod events;
pub mod iteration;
pub mod lifecycle;
pub mod observer;
pub mod policy;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{Instrument, field};

use crate::agent::{Agent, AgentError, AgentRun};
use crate::prompt::{Prompt, PromptSelector};
use crate::queue::Queue;
use crate::signal::{Signal, SignalId};
use crate::time::{Clock, IdSource};
use crate::variable::VariableStore;
use crate::workspace::{ActiveWorkspace, Workspace};

pub use builder::{BuilderError, RunnerBuilder};
pub use completion::{
    CompletionCondition, CompletionConditionErrorPolicy, CompletionConditionInfo,
    CompletionConditionKind, CompletionEvent, CompletionRequest, RunnerExit,
};
pub use error::{ErrorSource, RunnerError};
pub use event::{EventName, HookEvent, SharedSignal};
pub use event_emitter::EventDispatcher;
pub use event_handler::{BoxError, EventAction};
pub use iteration::{IterationContext, IterationState, PriorIterationStatus};
pub use lifecycle::{RedactedMetadata, RunnerLifecycleEvent};
pub use observer::{DynRunnerObserver, ObserveFuture, RunnerObserver};
pub use policy::{RunnerPolicy, RunnerTerminationReason, SignalAcquisition};

use completion::CompletionTracker;
use events::RunnerEmitter;

/// Drives a queue of signals through a workspace and agent.
///
/// The runner is consumed by [`Runner::run`] so that owned state can be
/// moved into the loop.
///
/// `queue` is `Option`: a runner configured with `behavior = loop` may
/// operate without a queue, synthesising signals on each iteration. The
/// builder rejects the inconsistent `(queue=None, behavior=Wait)`
/// combination.
pub struct Runner {
    pub(crate) queue: Option<Arc<dyn Queue>>,
    /// The one workspace bound to this runner for the whole exploration.
    /// Each iteration brackets it with `setup` → agent run → the active
    /// workspace's `teardown`.
    pub(crate) workspace: Box<dyn Workspace>,
    pub(crate) agent: Agent,
    pub(crate) prompt_selector: PromptSelector,
    pub(crate) events: EventDispatcher,
    pub(crate) config: RunnerPolicy,
    pub(crate) completion_conditions: Vec<CompletionCondition>,
    /// System-contract observer fan-out.
    ///
    /// Each registered observer receives the
    /// [`RunnerLifecycleEvent`] projection of every lifecycle `HookEvent` *before*
    /// the user-defined `events` emitter sees it. Observer errors are
    /// tallied separately into the terminal `runner_finished` event; they
    /// never block runner progress.
    pub(crate) observers: Vec<Arc<dyn DynRunnerObserver>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) id_source: Arc<dyn IdSource>,
    pub(crate) variables: VariableStore,
    /// Private scratch area retained for the lifetime of this Runner.
    ///
    /// Agent command artifacts belong here, never in a Workspace.
    pub(crate) temporary_directory: tempfile::TempDir,
}

impl Runner {
    /// Start a fluent [`RunnerBuilder`].
    pub fn builder() -> RunnerBuilder {
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
    /// * a configured completion condition requests a safe exit,
    /// * `once` is set in [`RunnerPolicy`] and one signal was processed, or
    /// * a processing error occurs and `continue_on_error` is `false`.
    ///
    /// When `behavior = loop` is configured the runner synthesises a
    /// signal each time the queue is empty (or whenever it has no queue),
    /// applying the configured `delay` between successive synthesised
    /// iterations. The first iteration runs without delay so a one-shot
    /// `behavior = loop` invocation starts immediately.
    pub async fn run(self, cancel: CancellationToken) -> Result<RunnerExit, RunnerError> {
        let Runner {
            queue,
            mut workspace,
            agent,
            prompt_selector,
            events: emitter,
            config,
            completion_conditions,
            observers,
            clock,
            id_source,
            variables,
            temporary_directory,
        } = self;
        let mut events = RunnerEmitter::new(emitter, observers);
        let runner_started_at = clock.now();
        let mut iter_state = IterationState::new(runner_started_at);
        let mut iteration_count: u32 = 0;
        let mut last_signal_id: Option<SignalId> = None;
        let completion = CompletionTracker::new(
            completion_conditions,
            runner_started_at,
            clock.monotonic_now(),
        );

        events.bootstrap(runner_started_at).await;
        let bootstrap_snapshot = iter_state.snapshot(0);
        events.runner_starting(&bootstrap_snapshot).await;

        let loop_result = run_loop(
            queue.as_deref(),
            workspace.as_mut(),
            &agent,
            &prompt_selector,
            &config,
            &completion,
            &cancel,
            clock.as_ref(),
            id_source.as_ref(),
            &variables,
            temporary_directory.path(),
            &mut events,
            &mut iter_state,
            &mut iteration_count,
            &mut last_signal_id,
        )
        .await;

        let final_reason = match &loop_result {
            Ok(reason) => reason.clone(),
            Err(err) => RunnerTerminationReason::Error {
                error_source: err.error_source(),
                message: err.message().to_owned(),
            },
        };
        let runner_finished_snapshot = iter_state.snapshot(iteration_count);
        if let RunnerTerminationReason::Completed { request } = &final_reason {
            events
                .runner_completing(request.clone(), &runner_finished_snapshot)
                .await;
        }
        events
            .runner_finished(
                final_reason,
                iteration_count,
                last_signal_id,
                &runner_finished_snapshot,
            )
            .await;

        loop_result.map(|reason| RunnerExit {
            reason,
            iteration_count,
            last_signal_id,
            iteration: runner_finished_snapshot,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Composition primitives for `Runner::run`.
//
// Each concern below holds exactly one responsibility: `RunnerEmitter`
// (in events.rs) owns the broadcast + tally pair; `IterationFailure`
// and `NextSignal` are the data shapes that compose processing results;
// `decide_after_processing_failure` is the pure failure-policy decision;
// the `next_signal` / `render_prompt` / `drive_workspace` functions are
// typed and side-effect-explicit.  Each function receives only the
// parameters it actually uses.
// ─────────────────────────────────────────────────────────────────────────

type BoxedError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Pre-iteration failure from a queue `dequeue` call.
///
/// Distinct from [`IterationFailure`] because dequeue errors are
/// handled asymmetrically by the run loop: they do **not** bump the
/// iteration counter, do **not** update streak state, and are **not**
/// subject to the `once` policy.
struct DequeueError {
    message: String,
    source: BoxedError,
}

#[derive(Debug)]
struct InvalidSignalAcquisition {
    message: &'static str,
}

impl std::fmt::Display for InvalidSignalAcquisition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for InvalidSignalAcquisition {}

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

    fn into_error(self) -> RunnerError {
        RunnerError {
            error_source: ErrorSource::Dequeue,
            message: self.message,
            source: self.source,
        }
    }
}

/// Typed failure produced during signal processing (post-dequeue).
///
/// One shape carrying the [`ErrorSource`] that says which operation failed;
/// the failing-operation classification is a single field, not a variant set.
struct IterationFailure {
    error_source: ErrorSource,
    signal_id: SignalId,
    source: BoxedError,
    message: String,
    /// Process exit code, available only for an [`ErrorSource::AgentRun`]
    /// failure.
    exit: Option<i32>,
}

impl IterationFailure {
    fn signal_id(&self) -> SignalId {
        self.signal_id
    }

    fn exit(&self) -> Option<i32> {
        self.exit
    }

    fn message(&self) -> &str {
        &self.message
    }

    fn error_source(&self) -> ErrorSource {
        self.error_source
    }

    fn into_error(self) -> RunnerError {
        RunnerError {
            error_source: self.error_source,
            message: self.message,
            source: self.source,
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
fn decide_after_processing_failure(cfg: &RunnerPolicy) -> FailureDecision {
    if !cfg.continue_on_error {
        return FailureDecision::Bubble;
    }
    if cfg.once {
        return FailureDecision::Once;
    }
    FailureDecision::Retry
}

/// Acquire the next signal: park on the queue, race a non-blocking
/// dequeue against synthesise, or synthesise outright depending on
/// `(queue, behavior)`. Pure acquisition — no events, no I/O on the
/// emitter.
async fn next_signal(
    queue: Option<&dyn Queue>,
    behavior: &SignalAcquisition,
    cancel: &CancellationToken,
    iteration_count: u32,
    clock: &dyn Clock,
    id_source: &dyn IdSource,
) -> NextSignal {
    match (queue, behavior) {
        (Some(queue), SignalAcquisition::Wait) => {
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
        (Some(queue), SignalAcquisition::Synthesize { delay }) => {
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
                    NextSignal::Got(Signal::synthesized_with_sources(clock, id_source))
                }
                Err(err) => NextSignal::Failed(DequeueError::new(err)),
            }
        }
        (None, SignalAcquisition::Synthesize { delay }) => {
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
            NextSignal::Got(Signal::synthesized_with_sources(clock, id_source))
        }
        (None, SignalAcquisition::Wait) => {
            NextSignal::Failed(DequeueError::new(InvalidSignalAcquisition {
                message: "(queue=None, behavior=Wait) is rejected at builder time",
            }))
        }
    }
}

fn render_prompt(
    selector: &PromptSelector,
    signal: &Signal,
    snap: &IterationContext,
    signal_id: SignalId,
    variables: &VariableStore,
) -> Result<Prompt, IterationFailure> {
    selector
        .render_with_variables(signal, snap, variables)
        .map_err(|err| {
            let message = err.to_string();
            IterationFailure {
                error_source: ErrorSource::RenderPrompt,
                signal_id,
                source: Box::new(err),
                message,
                exit: None,
            }
        })
}

/// Successful agent run record — carried out of `drive_workspace` so
/// the caller can finalise iteration state without re-deriving anything
/// from the report.
struct AgentRecord {
    exit_code: Option<i32>,
}

/// Upper bound on how long the drain window waits for the agent future
/// after `iteration_timeout` fires. Derived from the agent-side
/// SIGTERM grace so the drain always exceeds it; if you change one, the
/// other follows automatically.
const ITERATION_TIMEOUT_DRAIN_GRACE: Duration =
    Duration::from_secs(crate::agent::process::AGENT_TERMINATION_GRACE.as_secs() + 5);

/// Run the agent with the runner's optional iteration timeout.
///
/// Creates a child cancellation token from `cancel` for the agent. When
/// `timeout` is `Some(limit)`, the agent future is raced against the
/// timeout. On expiry the child token is cancelled, giving the agent up to
/// [`ITERATION_TIMEOUT_DRAIN_GRACE`] to shut down gracefully. During the
/// drain window, the parent `cancel` token is also watched so an operator
/// Ctrl-C doesn't hang.
///
/// The agent future is pinned across the timeout boundary. On the normal
/// drain path it is polled to completion — so a graceful shutdown (and the
/// driver's own `cleanup`) is never cut short by a synchronous
/// `ProcessGroup::Drop`. Two drain exits are deliberate exceptions: an
/// operator Ctrl-C during the drain, or the drain grace being exceeded,
/// each returns while the future is still pending and drops it. That drop
/// fires `ProcessGroup::Drop`, which sends `SIGKILL` to the process group
/// synchronously as an OS-level backstop; the agent's async `cleanup` does not run on that
/// forced path. Bounding the wait is the point — blocking indefinitely on a
/// stuck agent would hang the operator. (`BackupSlot::snapshot` is
/// idempotent by capture, so a skipped hook finalize cannot corrupt a later
/// install.)
async fn run_agent_with_timeout(
    agent: &Agent,
    workspace: &dyn ActiveWorkspace,
    temporary_directory: &Path,
    prompt: &Prompt,
    cancel: &CancellationToken,
    timeout: Option<Duration>,
) -> Result<AgentRun, AgentError> {
    let iter_cancel = cancel.child_token();
    match timeout {
        Some(limit) => {
            let mut agent_fut = std::pin::pin!(agent.run_on_with_temporary_directory(
                workspace,
                temporary_directory,
                prompt,
                iter_cancel.clone(),
            ));
            tokio::select! {
                biased;
                res = agent_fut.as_mut() => res,
                () = tokio::time::sleep(limit) => {
                    iter_cancel.cancel();
                    tokio::select! {
                        biased;
                        _ = agent_fut.as_mut() => {}
                        () = cancel.cancelled() => {}
                        () = tokio::time::sleep(ITERATION_TIMEOUT_DRAIN_GRACE) => {}
                    }
                    Err(AgentError::IterationTimeout(limit))
                }
            }
        }
        None => {
            agent
                .run_on_with_temporary_directory(
                    workspace,
                    temporary_directory,
                    prompt,
                    iter_cancel,
                )
                .await
        }
    }
}

/// Best-effort workspace cleanup after an agent-run failure.
///
/// Consumes the active workspace without emitting lifecycle events (the
/// silent-teardown contract); the persistent path it returns is discarded.
/// If teardown also fails, logs via `tracing::warn!`.
async fn best_effort_teardown(
    active: Box<dyn ActiveWorkspace>,
    signal_id: SignalId,
    failed_operation: ErrorSource,
    cancel: &CancellationToken,
) {
    if let Err(teardown_err) = active.teardown(cancel.clone()).await {
        let message = teardown_err.to_string();
        let span = tracing::Span::current();
        iter_tracing::record_span_error(&span, ErrorSource::WorkspaceTeardown.as_str(), &message);
        tracing::warn!(
            signal_id = %signal_id,
            failed_operation = failed_operation.as_str(),
            error = %message,
            "best-effort workspace teardown after failure returned an \
             error; workspace may not be fully cleaned up",
        );
    }
}

/// Drive the workspace bracket — setup -> agent -> teardown — for one
/// signal, emitting lifecycle events as it goes.
async fn drive_workspace(
    workspace: &mut dyn Workspace,
    agent: &Agent,
    temporary_directory: &Path,
    config: &RunnerPolicy,
    cancel: &CancellationToken,
    events: &mut RunnerEmitter,
    signal: &SharedSignal,
    prompt: &Prompt,
    snap: &IterationContext,
) -> Result<AgentRecord, IterationFailure> {
    let signal_id = signal.id();
    let workspace_name = workspace.name();

    events.workspace_setup_starting(signal, snap).await;

    let setup_span = tracing::info_span!(
        "iter.workspace.setup",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        iter.workspace.name = workspace_name,
        iter.workspace.path = field::Empty,
    );
    // Setup either yields the active workspace or has self-cleaned; a
    // failed setup leaves nothing to tear down.
    let active = match workspace
        .setup(cancel.clone())
        .instrument(setup_span.clone())
        .await
    {
        Ok(active) => active,
        Err(err) => {
            let message = err.to_string();
            iter_tracing::record_span_error(
                &setup_span,
                ErrorSource::WorkspaceSetup.as_str(),
                &message,
            );
            events
                .runner_error(
                    ErrorSource::WorkspaceSetup,
                    Some(signal_id),
                    &message,
                    HookEvent::WorkspaceSetupFailed {
                        signal_id,
                        error: message.clone(),
                    },
                    snap,
                )
                .await;
            return Err(IterationFailure {
                error_source: ErrorSource::WorkspaceSetup,
                signal_id,
                source: Box::new(err),
                message,
                exit: None,
            });
        }
    };

    let workspace_path = active.path().to_path_buf();
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

    let agent_span = tracing::info_span!(
        "iter.agent.run",
        iter.signal.id = %signal_id,
        iter.signal.kind = %signal.kind(),
        // Recorded by the agent per attempt: the label of the driver that
        // actually ran (a fallback sequence leaves its final attempt).
        iter.agent.name = field::Empty,
        iter.workspace.path = %workspace_path_attr.display(),
        iter.prompt.bytes = prompt.as_str().len(),
        iter.agent.result = field::Empty,
        iter.agent.exit_code = field::Empty,
        iter.agent.exit_disposition = field::Empty,
    );
    let agent_result = run_agent_with_timeout(
        agent,
        &*active,
        temporary_directory,
        prompt,
        cancel,
        config.iteration_timeout,
    )
    .instrument(agent_span.clone())
    .await;

    // The agent result is now a plain `Result`: `Ok` means the agent ran
    // (exit 0), `Err` carries the failure class. The lifecycle label and
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
            ErrorSource::AgentRun.as_str(),
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
        iter_tracing::record_span_error(&agent_span, ErrorSource::AgentRun.as_str(), &message);
        events
            .runner_error(
                ErrorSource::AgentRun,
                Some(signal_id),
                &message,
                HookEvent::AgentRunFailed {
                    signal_id,
                    error: message.clone(),
                },
                snap,
            )
            .await;
        best_effort_teardown(active, signal_id, ErrorSource::AgentRun, cancel).await;
        return Err(IterationFailure {
            error_source: ErrorSource::AgentRun,
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
    // Teardown consumes the active workspace and returns the persistent
    // path — the durable location of the agent's work, carried on the
    // teardown-finished event for post-teardown handlers.
    let final_path = match active
        .teardown(cancel.clone())
        .instrument(teardown_span.clone())
        .await
    {
        Ok(final_path) => final_path,
        Err(err) => {
            let message = err.to_string();
            iter_tracing::record_span_error(
                &teardown_span,
                ErrorSource::WorkspaceTeardown.as_str(),
                &message,
            );
            events
                .runner_error(
                    ErrorSource::WorkspaceTeardown,
                    Some(signal_id),
                    &message,
                    HookEvent::WorkspaceTeardownFailed {
                        signal_id,
                        error: message.clone(),
                    },
                    snap,
                )
                .await;
            return Err(IterationFailure {
                error_source: ErrorSource::WorkspaceTeardown,
                signal_id,
                source: Box::new(err),
                message,
                exit: None,
            });
        }
    };
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
async fn run_iteration(
    workspace: &mut dyn Workspace,
    agent: &Agent,
    temporary_directory: &Path,
    prompt_selector: &PromptSelector,
    config: &RunnerPolicy,
    cancel: &CancellationToken,
    clock: &dyn Clock,
    events: &mut RunnerEmitter,
    iter_state: &mut IterationState,
    iteration_count: u32,
    signal: Signal,
    variables: &VariableStore,
) -> Result<(), IterationFailure> {
    // Wrap the dequeued signal in the shared, immutable handle exactly once,
    // before any lifecycle event is emitted. Every event for this bracket then
    // carries a cheap `Arc` clone of it rather than a deep copy of the signal.
    // Runner control flow still operates on the signal by reference: it derefs
    // to `&Signal`, and `signal.as_signal()` hands the bare `&Signal` to the
    // functions (e.g. `render_prompt`) that require it.
    let signal = SharedSignal::new(signal);
    let now = clock.now();
    iter_state.begin_iteration(now);
    let snap = iter_state.snapshot(iteration_count + 1);
    let signal_id = signal.id();

    events.signal_received(&signal, now, &snap).await;
    let prompt = match render_prompt(
        prompt_selector,
        signal.as_signal(),
        &snap,
        signal_id,
        variables,
    ) {
        Ok(p) => p,
        Err(failure) => {
            events
                .runner_error(
                    failure.error_source(),
                    Some(failure.signal_id()),
                    failure.message(),
                    HookEvent::RenderPromptFailed {
                        signal_id: failure.signal_id(),
                        error: failure.message().to_owned(),
                    },
                    &snap,
                )
                .await;
            return Err(failure);
        }
    };
    let record = drive_workspace(
        workspace,
        agent,
        temporary_directory,
        config,
        cancel,
        events,
        &signal,
        &prompt,
        &snap,
    )
    .await?;
    iter_state.record_success(signal_id, record.exit_code, clock.now());
    Ok(())
}

/// Drive repetition + termination policy.
///
/// Treats dequeue failures and processing failures **asymmetrically**:
/// dequeue failures do NOT bump `iteration_count` and do NOT call
/// `iter_state.record_failure` — they happen pre-iteration. Only
/// `run_iteration` errors bump the counter and update streak state.
async fn run_loop(
    queue: Option<&dyn Queue>,
    workspace: &mut dyn Workspace,
    agent: &Agent,
    prompt_selector: &PromptSelector,
    config: &RunnerPolicy,
    completion: &CompletionTracker,
    cancel: &CancellationToken,
    clock: &dyn Clock,
    id_source: &dyn IdSource,
    variables: &VariableStore,
    temporary_directory: &Path,
    events: &mut RunnerEmitter,
    iter_state: &mut IterationState,
    iteration_count: &mut u32,
    last_signal_id: &mut Option<SignalId>,
) -> Result<RunnerTerminationReason, RunnerError> {
    loop {
        if cancel.is_cancelled() {
            return Ok(RunnerTerminationReason::Cancelled);
        }
        if let Some(request) = completion.due_time_request(
            *iteration_count,
            *last_signal_id,
            clock.now(),
            clock.monotonic_now(),
        ) {
            return Ok(RunnerTerminationReason::Completed { request });
        }

        // Pre-iteration snapshot (count = iteration_count + 1) so a
        // dequeue-failure `runner_error` hook still sees the iteration
        // number that *would* have run.
        let snap = iter_state.snapshot(*iteration_count + 1);

        let next = next_signal(
            queue,
            &config.behavior,
            cancel,
            *iteration_count,
            clock,
            id_source,
        );
        tokio::pin!(next);
        let next = if let Some((condition_index, deadline)) = completion.next_time_condition() {
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    let request = completion.time_request(
                        condition_index,
                        *iteration_count,
                        *last_signal_id,
                        clock.now(),
                        clock.monotonic_now(),
                    );
                    return Ok(RunnerTerminationReason::Completed { request });
                }
                next = &mut next => next,
            }
        } else {
            next.await
        };

        match next {
            NextSignal::Drained => {
                return Ok(RunnerTerminationReason::QueueDrained);
            }
            NextSignal::Cancelled => {
                return Ok(RunnerTerminationReason::Cancelled);
            }
            NextSignal::Failed(dequeue_err) => {
                events
                    .runner_error(
                        ErrorSource::Dequeue,
                        None,
                        dequeue_err.message(),
                        HookEvent::DequeueFailed {
                            error: dequeue_err.message().to_owned(),
                        },
                        &snap,
                    )
                    .await;
                if !config.continue_on_error {
                    return Err(dequeue_err.into_error());
                }
            }
            NextSignal::Got(signal) if signal.is_terminate() => {
                *last_signal_id = Some(signal.id());
                return Ok(RunnerTerminationReason::TerminateSignalReceived);
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
                // Open the ambient iteration scope around the whole
                // iteration future: setup, the agent (and its child-process
                // env injection), and teardown all read the signal
                // correlation attributes from here instead of carrying them
                // through call signatures.
                let iteration_attrs = iter_tracing::IterationAttrs::new(
                    signal_id.to_string(),
                    signal.kind().to_string(),
                );
                let (iteration_result, latched_completion) = {
                    let iteration = iter_tracing::iteration_scope(
                        iteration_attrs,
                        run_iteration(
                            workspace,
                            agent,
                            temporary_directory,
                            prompt_selector,
                            config,
                            cancel,
                            clock,
                            events,
                            iter_state,
                            *iteration_count,
                            signal,
                            variables,
                        )
                        .instrument(span.clone()),
                    );
                    tokio::pin!(iteration);
                    let mut latched_completion = None;
                    let result = if let Some((condition_index, deadline)) =
                        completion.next_time_condition()
                    {
                        tokio::select! {
                            biased;
                            result = &mut iteration => result,
                            () = tokio::time::sleep_until(deadline) => {
                                latched_completion = Some(completion.time_request(
                                    condition_index,
                                    *iteration_count,
                                    *last_signal_id,
                                    clock.now(),
                                    clock.monotonic_now(),
                                ));
                                iteration.as_mut().await
                            }
                        }
                    } else {
                        iteration.as_mut().await
                    };
                    (result, latched_completion)
                };

                let mut once_after_iteration = false;
                match iteration_result {
                    Ok(()) => {
                        span.record("iter.runner.result", "success");
                        *iteration_count += 1;
                        once_after_iteration = config.once;
                    }
                    Err(failure) => {
                        span.record("iter.runner.result", "failure");
                        iter_tracing::record_span_error(
                            &span,
                            failure.error_source().as_str(),
                            failure.message(),
                        );
                        iter_state.record_failure(failure.signal_id(), failure.exit(), clock.now());
                        *iteration_count += 1;
                        match decide_after_processing_failure(config) {
                            FailureDecision::Retry => {}
                            FailureDecision::Once => {
                                once_after_iteration = true;
                            }
                            FailureDecision::Bubble => {
                                return Err(failure.into_error());
                            }
                        }
                    }
                }

                let latched_completion = latched_completion.map(|mut request| {
                    request.iteration_count = *iteration_count;
                    request.last_signal_id = *last_signal_id;
                    request
                });
                let boundary = match completion
                    .evaluate_boundary(
                        *iteration_count,
                        *last_signal_id,
                        clock.now(),
                        clock.monotonic_now(),
                    )
                    .await
                {
                    Ok(boundary) => boundary,
                    Err(source) => {
                        let message = source.to_string();
                        let failure_event = HookEvent::CompletionConditionFailed {
                            condition_name: source.condition_name().to_owned(),
                            error: source.message().to_owned(),
                        };
                        let final_snapshot = iter_state.snapshot(*iteration_count);
                        events
                            .runner_error(
                                ErrorSource::CompletionCondition,
                                *last_signal_id,
                                &message,
                                failure_event,
                                &final_snapshot,
                            )
                            .await;
                        return Err(RunnerError {
                            error_source: ErrorSource::CompletionCondition,
                            message,
                            source: Box::new(source),
                        });
                    }
                };
                if let Some(request) = boundary {
                    if let Some(latched) = latched_completion
                        && latched.condition == request.condition
                    {
                        // Boundary evaluation preserves declaration-order
                        // precedence. Reuse the timer's original request
                        // timestamp when it is the winning condition.
                        return Ok(RunnerTerminationReason::Completed { request: latched });
                    }
                    return Ok(RunnerTerminationReason::Completed { request });
                }
                if let Some(request) = latched_completion {
                    // Defensive fallback: the latched time condition should
                    // also be due during boundary evaluation.
                    return Ok(RunnerTerminationReason::Completed { request });
                }
                if once_after_iteration {
                    return Ok(RunnerTerminationReason::Once);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;
