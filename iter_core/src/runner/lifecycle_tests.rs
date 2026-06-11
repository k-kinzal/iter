//! Exact-once emission tests for the runner-level
//! `RunnerStarting` / `RunnerFinished` events across every
//! termination path. These are the load-bearing tests for the
//! labeled-`'run_loop` design: a regression that adds a `return`
//! escape hatch (or a new `break 'run_loop` site that bypasses the
//! post-loop emit) would silently drop one of the events and these
//! tests would catch it.
//!
//! Each test uses a `CapturingHandler` that pushes every received
//! `HookEvent` into a shared `Vec`, and asserts:
//!   * `RunnerStarting` appears exactly once,
//!   * `RunnerFinished` appears exactly once,
//!   * the `RunnerFinished` `reason` matches the expected
//!     termination reason.
//!
//! The signal-processing steps are stubbed via fake `Workspace`
//! and `Agent` impls. We rely on `InMemoryQueue` for the queue.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::agent::{AgentInvocation, AgentRun};
use crate::prompt::PromptTemplate;
use crate::queue::{InMemoryQueue, Priority, QueueError};
use crate::signal::{Metadata, Signal};
use crate::workspace::WorkspaceError;
use async_trait::async_trait;

struct FakeWorkspace {
    path: PathBuf,
}

#[async_trait]
impl Workspace for FakeWorkspace {
    async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Ok(())
    }
    async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Ok(())
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn final_path(&self) -> &Path {
        self.path()
    }
    fn name(&self) -> &'static str {
        "fake"
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

#[async_trait]
impl Workspace for FailingTeardownWorkspace {
    async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Ok(())
    }
    async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        self.teardown_calls.fetch_add(1, Ordering::SeqCst);
        Err(WorkspaceError::new(std::io::Error::other("boom")))
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn final_path(&self) -> &Path {
        self.path()
    }
    fn name(&self) -> &'static str {
        "failing-teardown"
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

#[async_trait]
impl Workspace for FailingSetupWorkspace {
    async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Err(WorkspaceError::new(std::io::Error::other("setup-boom")))
    }
    async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        self.teardown_calls.fetch_add(1, Ordering::SeqCst);
        Err(WorkspaceError::new(std::io::Error::other("teardown-boom")))
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn final_path(&self) -> &Path {
        self.path()
    }
    fn name(&self) -> &'static str {
        "failing-setup"
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

#[async_trait]
impl Workspace for TransientWorkspace {
    async fn setup(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Ok(())
    }
    async fn teardown(&mut self, _cancel: CancellationToken) -> Result<(), WorkspaceError> {
        Ok(())
    }
    fn path(&self) -> &Path {
        self.temp.path()
    }
    fn final_path(&self) -> &Path {
        &self.base
    }
    fn name(&self) -> &'static str {
        "transient"
    }
}

#[derive(Default)]
struct StubAgent;

#[async_trait]
impl Agent for StubAgent {
    async fn run(&self, _ctx: AgentInvocation<'_>) -> Result<AgentRun, crate::agent::AgentError> {
        Ok(AgentRun::empty())
    }
}

/// Agent that always returns an error. Used to assert the
/// agent-failure event-ordering contract: `RunnerError(AgentRun)`
/// fires immediately after `AgentFinished` and `WorkspaceTeardown*`
/// events are suppressed for the failing iteration.
struct FailingAgent;

#[async_trait]
impl Agent for FailingAgent {
    async fn run(&self, _ctx: AgentInvocation<'_>) -> Result<AgentRun, crate::agent::AgentError> {
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

#[async_trait]
impl Agent for SluggishCleanupAgent {
    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, crate::agent::AgentError> {
        ctx.cancel.cancelled().await;
        // Stay alive much longer than DRAIN_GRACE so that any test
        // that bounded its overall wait at < DRAIN_GRACE could only
        // succeed if the runner short-circuited the drain.
        tokio::time::sleep(self.post_cancel_sleep).await;
        Err(crate::agent::AgentError::Cancelled)
    }
}

#[async_trait]
impl Agent for SleepyAgent {
    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, crate::agent::AgentError> {
        tokio::select! {
            () = tokio::time::sleep(self.delay) => Ok(AgentRun::empty()),
            () = ctx.cancel.cancelled() => {
                self.cancel_observed.store(true, Ordering::SeqCst);
                Err(crate::agent::AgentError::Cancelled)
            }
        }
    }
}

/// Captures every `HookEvent` the runner emits. Wrapped in `Arc<Mutex>`
/// so the test can read it back after the runner returns.
#[derive(Default, Clone)]
struct CapturingHandler {
    events: Arc<Mutex<Vec<HookEvent>>>,
}

impl EventAction for CapturingHandler {
    async fn handle(
        &self,
        event: &HookEvent,
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
    events: Arc<Mutex<Vec<HookEvent>>>,
    calls: Arc<AtomicUsize>,
}

impl EventAction for FailFirstHandler {
    async fn handle(
        &self,
        event: &HookEvent,
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

fn make_provider() -> impl Fn() -> Box<dyn Workspace> + Send + Sync {
    || -> Box<dyn Workspace> {
        Box::new(FakeWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
        })
    }
}

fn make_failing_teardown_provider() -> (
    impl Fn() -> Box<dyn Workspace> + Send + Sync,
    Arc<AtomicUsize>,
) {
    let teardown_calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&teardown_calls);
    let factory = move || -> Box<dyn Workspace> {
        Box::new(FailingTeardownWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
            teardown_calls: Arc::clone(&counter),
        })
    };
    (factory, teardown_calls)
}

fn make_failing_setup_provider() -> (
    impl Fn() -> Box<dyn Workspace> + Send + Sync,
    Arc<AtomicUsize>,
) {
    let teardown_calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&teardown_calls);
    let factory = move || -> Box<dyn Workspace> {
        Box::new(FailingSetupWorkspace {
            path: PathBuf::from("/tmp/iter-runner-test"),
            teardown_calls: Arc::clone(&counter),
        })
    };
    (factory, teardown_calls)
}

fn count_runner_starting(events: &[HookEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, HookEvent::RunnerStarting {}))
        .count()
}

fn count_runner_finished(events: &[HookEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, HookEvent::RunnerFinished { .. }))
        .count()
}

fn finished_reason(events: &[HookEvent]) -> RunnerTerminationReason {
    for e in events {
        if let HookEvent::RunnerFinished { reason, .. } = e {
            return reason.clone();
        }
    }
    panic!("no RunnerFinished in events: {events:?}");
}

#[tokio::test]
async fn once_path_emits_runner_starting_and_finished_exactly_once() {
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    let handler = CapturingHandler::default();
    let cancel = CancellationToken::new();
    // Pre-cancel so the runner short-circuits on the first
    // `cancel.is_cancelled()` check at the top of the loop.
    cancel.cancel();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    // Close immediately so dequeue returns None and the runner
    // takes the QueueDrained exit branch.
    Queue::close(queue.as_ref()).await.unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
async fn error_path_emits_runner_starting_and_finished_exactly_once() {
    // Workspace teardown fails with continue_on_error=false →
    // `RunnerExitError`. The post-loop block must
    // still emit RunnerFinished with an Error reason.
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let (provider, _teardown_calls) = make_failing_teardown_provider();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
        matches!(
            &err,
            RunnerExitError {
                error_source: ErrorSource::WorkspaceTeardown,
                ..
            }
        ),
        "expected workspace_teardown error source, got {err:?}"
    );

    let events = handler.events.lock().unwrap().clone();
    assert_eq!(count_runner_starting(&events), 1, "starting once on Err");
    assert_eq!(count_runner_finished(&events), 1, "finished once on Err");
    let reason = finished_reason(&events);
    match reason {
        RunnerTerminationReason::Error { error_source, .. } => {
            assert_eq!(error_source, ErrorSource::WorkspaceTeardown);
        }
        other => panic!("expected Error reason, got {other:?}"),
    }
}

#[tokio::test]
async fn error_path_carries_handler_counts_in_exit_error() {
    // The `Err` exit path now propagates handler/observer counts
    // via error variant fields, mirroring the `Ok` path's
    // `RunnerSummary` fields. This test asserts the count survives.
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let events_buf: Arc<Mutex<Vec<HookEvent>>> = Arc::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = FailFirstHandler {
        events: Arc::clone(&events_buf),
        calls: Arc::clone(&calls),
    };
    let (provider, _teardown_calls) = make_failing_teardown_provider();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
            iteration_timeout: None,
        })
        .on_all(handler)
        .build()
        .unwrap();

    let err = runner
        .run(CancellationToken::new())
        .await
        .expect_err("teardown failure becomes a workspace_teardown error");
    assert_eq!(
        err.error_source,
        ErrorSource::WorkspaceTeardown,
        "expected workspace_teardown error source, got {err:?}"
    );
    assert!(
        err.event_handler_error_count >= 1,
        "the failing first handler invocation must be counted",
    );
}

/// Captures `(HookEvent, IterationContext)` pairs for every emit so
/// tests can assert on the iteration snapshot the runner threaded
/// through alongside each event. The matching `CapturingHandler`
/// drops the iteration argument; tests that need to inspect it use
/// this one.
#[derive(Default, Clone)]
struct CapturingIterHandler {
    events: Arc<Mutex<Vec<(HookEvent, IterationContext)>>>,
}

impl EventAction for CapturingIterHandler {
    async fn handle(
        &self,
        event: &HookEvent,
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
async fn teardown_failure_with_continue_on_error_carries_errored_result_to_next_iter() {
    // With `continue_on_error = true`, a teardown failure must record
    // a failure on the iteration accumulator before bumping the
    // counter — so the *next* iteration's prompt/template sees
    // `iteration.previous_result == "errored"` and
    // `consecutive_failures >= 1`. Regression for the open-coded
    // teardown branch that previously fell through to
    // `record_success`, which mis-marked the failed turn as a win
    // for the next snapshot.
    let inner = Arc::new(InMemoryQueue::new());
    let queue: Arc<dyn Queue> = inner.clone();
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingIterHandler::default();
    let (provider, _teardown_calls) = make_failing_teardown_provider();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
            iteration_timeout: None,
        })
        .on_all(handler.clone())
        .build()
        .unwrap();

    // Wait blocks once the queue drains; cancel after both signals
    // have been processed.
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let queue_for_task = Arc::clone(&inner);
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
    // errored result forward.
    let second_agent_starting = events
        .iter()
        .filter(|(e, _)| matches!(e, HookEvent::AgentStarting { .. }))
        .nth(1)
        .map(|(_, iter)| iter.clone())
        .expect("a second AgentStarting must have been emitted");
    assert_eq!(
        second_agent_starting.previous_result,
        PreviousResult::Errored,
        "the failed teardown of iter 1 must have flipped previous_result",
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let events_buf: Arc<Mutex<Vec<HookEvent>>> = Arc::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = FailFirstHandler {
        events: Arc::clone(&events_buf),
        calls: Arc::clone(&calls),
    };
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(SleepyAgent::new(Duration::from_secs(60))))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
            iteration_timeout: Some(Duration::from_millis(150)),
        })
        .on_all(handler.clone())
        .build()
        .unwrap();

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), runner.run(CancellationToken::new()))
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
        HookEvent::AgentRunFailed { error, .. } => error.contains("iteration"),
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
            HookEvent::DequeueFailed { .. }
                | HookEvent::RenderPromptFailed { .. }
                | HookEvent::WorkspaceSetupFailed { .. }
                | HookEvent::AgentRunFailed { .. }
                | HookEvent::WorkspaceTeardownFailed { .. }
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
    // so we get a single iteration and observe its result via the
    // emitted `AgentFinished` event (`result label = "cancelled"`).
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(SleepyAgent::new(Duration::from_secs(60))))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
        .any(|e| matches!(e, HookEvent::AgentRunFailed { .. }));
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(agent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let runner = Runner::builder()
            .queue(Arc::clone(&queue))
            .workspaces(make_provider())
            // Outlast DRAIN_GRACE comfortably so the only way the test
            // returns quickly is via the parent-cancel arm.
            .agent(Box::new(SluggishCleanupAgent {
                post_cancel_sleep: Duration::from_secs(60),
            }))
            .prompt_template(PromptTemplate::new("hello").unwrap())
            .config(RunnerPolicy {
                once: true,
                continue_on_error: true,
                behavior: SignalAcquisition::Wait,
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(FailingAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
        .position(|e| matches!(e, HookEvent::AgentFinished { .. }))
        .expect("AgentFinished must fire even when the agent errored");
    let runner_error_idx = events
        .iter()
        .position(|e| matches!(e, HookEvent::AgentRunFailed { .. }))
        .expect("AgentRunFailed must fire after a failing agent");
    assert!(
        runner_error_idx > agent_finished_idx,
        "AgentRunFailed must follow AgentFinished, got events: {events:?}",
    );

    let runner_error_count = events
        .iter()
        .filter(|e| matches!(e, HookEvent::AgentRunFailed { .. }))
        .count();
    assert_eq!(
        runner_error_count, 1,
        "AgentRunFailed must fire exactly once per failed iteration, got: {events:?}",
    );

    let saw_teardown_event = events.iter().any(|e| {
        matches!(
            e,
            HookEvent::WorkspaceTeardownStarting { .. }
                | HookEvent::WorkspaceTeardownFinished { .. }
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let (provider, teardown_calls) = make_failing_setup_provider();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
        .filter(|e| matches!(e, HookEvent::WorkspaceSetupFailed { .. }))
        .count();
    assert_eq!(
        setup_error_count, 1,
        "WorkspaceSetupFailed must fire exactly once on setup failure, got: {events:?}",
    );

    let teardown_error_count = events
        .iter()
        .filter(|e| matches!(e, HookEvent::WorkspaceTeardownFailed { .. }))
        .count();
    assert_eq!(
        teardown_error_count, 0,
        "WorkspaceTeardownFailed must NOT fire when teardown is invoked as best-effort \
             cleanup after a setup failure (silent-teardown contract), got: {events:?}",
    );

    let saw_teardown_event = events.iter().any(|e| {
        matches!(
            e,
            HookEvent::WorkspaceTeardownStarting { .. }
                | HookEvent::WorkspaceTeardownFinished { .. }
        )
    });
    assert!(
        !saw_teardown_event,
        "Workspace teardown events must be suppressed on setup failure, got: {events:?}",
    );

    let saw_setup_finished = events
        .iter()
        .any(|e| matches!(e, HookEvent::WorkspaceSetupFinished { .. }));
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let (provider, teardown_calls) = make_failing_teardown_provider();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(FailingAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
        .filter(|e| matches!(e, HookEvent::AgentRunFailed { .. }))
        .count();
    assert_eq!(
        agent_error_count, 1,
        "AgentRunFailed must fire exactly once on agent failure, got: {events:?}",
    );

    let teardown_error_count = events
        .iter()
        .filter(|e| matches!(e, HookEvent::WorkspaceTeardownFailed { .. }))
        .count();
    assert_eq!(
        teardown_error_count, 0,
        "WorkspaceTeardownFailed must NOT fire when teardown is invoked as best-effort \
             cleanup after an agent failure (silent-teardown contract), got: {events:?}",
    );

    let saw_teardown_event = events.iter().any(|e| {
        matches!(
            e,
            HookEvent::WorkspaceTeardownStarting { .. }
                | HookEvent::WorkspaceTeardownFinished { .. }
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

#[async_trait]
impl Queue for FailingQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.inner.enqueue(signal, priority).await
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        let n = self.fail_count.fetch_add(1, Ordering::SeqCst);
        if n < self.max_failures {
            return Err(QueueError::new(std::io::Error::other("dequeue-boom")));
        }
        self.inner.dequeue(cancel).await
    }
}

#[tokio::test]
async fn dequeue_failure_without_continue_on_error_exits_with_error() {
    let queue: Arc<dyn Queue> = Arc::new(FailingQueue::new(1));
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: false,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
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
        matches!(
            err,
            RunnerExitError {
                error_source: ErrorSource::Dequeue,
                ..
            }
        ),
        "expected dequeue error source, got {err:?}"
    );

    let events = handler.events.lock().unwrap().clone();
    assert_eq!(count_runner_starting(&events), 1);
    assert_eq!(count_runner_finished(&events), 1);
    let finished_event = events
        .iter()
        .find(|e| matches!(e, HookEvent::RunnerFinished { .. }))
        .expect("RunnerFinished must be present");
    match finished_event {
        HookEvent::RunnerFinished {
            reason,
            iteration_count,
        } => {
            match reason {
                RunnerTerminationReason::Error { error_source, .. } => {
                    assert_eq!(error_source, &ErrorSource::Dequeue);
                }
                other => panic!("expected Error reason, got {other:?}"),
            }
            assert_eq!(
                *iteration_count, 0,
                "dequeue failures must not bump iteration_count"
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn dequeue_failure_with_continue_on_error_retries_and_does_not_bump_iteration() {
    let queue: Arc<dyn Queue> = Arc::new(FailingQueue::new(1));
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: true,
            behavior: SignalAcquisition::Wait,
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
        .filter(|e| matches!(e, HookEvent::DequeueFailed { .. }))
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::terminate(), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy::default())
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
    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    for _ in 0..3 {
        queue
            .enqueue(Signal::new(Metadata::new()), Priority::default())
            .await
            .unwrap();
    }
    queue
        .enqueue(Signal::terminate(), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(make_provider())
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy::default())
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
    let provider = move || -> Box<dyn Workspace> {
        Box::new(TransientWorkspace::new(persistent_path.clone()))
    };

    let queue: Arc<dyn Queue> = Arc::new(InMemoryQueue::new());
    queue
        .enqueue(Signal::new(Metadata::new()), Priority::default())
        .await
        .unwrap();
    let handler = CapturingHandler::default();
    let runner = Runner::builder()
        .queue(Arc::clone(&queue))
        .workspaces(provider)
        .agent(Box::new(StubAgent))
        .prompt_template(PromptTemplate::new("hello").unwrap())
        .config(RunnerPolicy {
            once: true,
            continue_on_error: false,
            behavior: SignalAcquisition::Wait,
            iteration_timeout: None,
        })
        .on_all(handler.clone())
        .build()
        .unwrap();

    runner.run(CancellationToken::new()).await.expect("run ok");

    let events = handler.events.lock().unwrap().clone();
    let teardown_path = events.iter().find_map(|e| match e {
        HookEvent::WorkspaceTeardownFinished { path, .. } => Some(path.clone()),
        _ => None,
    });
    assert_eq!(
        teardown_path.as_deref(),
        Some(expected.as_path()),
        "post-teardown event must carry the persistent path, not the transient working directory",
    );
}
