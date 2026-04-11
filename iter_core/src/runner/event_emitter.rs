//! [`EventEmitter`] — broadcasts [`Event`]s to a fan-out of registered
//! [`EventHandler`]s.
//!
//! Because the [`EventHandler`] trait uses RPITIT it is not directly
//! dyn-compatible. The emitter therefore wraps each handler in an internal
//! erased trait that returns a `Pin<Box<dyn Future>>` so a heterogeneous list
//! of handlers can be stored in a `Vec`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::warn;

use super::event::Event;
use super::event_handler::{BoxError, EventHandler};
use super::iteration::IterationContext;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Internal dyn-compatible adapter for [`EventHandler`].
trait DynEventHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        event: &'a Event,
        iteration: &'a IterationContext,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
}

struct DynAdapter<H: EventHandler> {
    inner: H,
}

impl<H: EventHandler> DynEventHandler for DynAdapter<H> {
    fn handle<'a>(
        &'a self,
        event: &'a Event,
        iteration: &'a IterationContext,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(self.inner.handle(event, iteration))
    }
}

/// Outcome of a single [`EventEmitter::emit`] call.
///
/// The emitter's contract is **best effort** — a failing handler is logged
/// and the remaining handlers still run — but silence is a hostile default
/// for workflow-critical handlers (e.g. a project-supplied
/// `on workspace_teardown_finished { shell "./scripts/persist-run.sh" }` that
/// persists results after teardown). This struct carries the observations
/// back to the caller so silence becomes visible:
/// the [`Runner`](crate::Runner) tallies `error_count` into
/// [`RunnerSummary::event_handler_error_count`](crate::RunnerSummary::event_handler_error_count)
/// and callers of [`EventEmitter::emit`] outside the runner can inspect
/// the count to decide whether to halt.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EmitReport {
    /// Number of handlers that returned `Err` while dispatching the event.
    ///
    /// Each error is also written to `tracing` at `warn` level with the
    /// handler index and error message, so downstream observers can
    /// reconstruct the detail even though only the count propagates here.
    pub error_count: usize,
}

impl EmitReport {
    /// `true` when every handler returned `Ok(())` (or no handlers were
    /// registered).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.error_count == 0
    }
}

/// Fan-out broadcaster of [`Event`]s.
///
/// Handlers are stored in registration order and invoked sequentially.
/// Failing handlers are logged via [`tracing`] at `warn` level and the
/// remaining handlers still run; callers read back the per-call error
/// count via [`EmitReport`] so a failing
/// `on workspace_teardown_finished { shell "..." }` can no longer vanish into the
/// void.
#[derive(Default, Clone)]
pub struct EventEmitter {
    handlers: Vec<Arc<dyn DynEventHandler>>,
}

impl EventEmitter {
    /// Build an emitter with no registered handlers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Handlers are invoked in registration order.
    pub fn register<H>(&mut self, handler: H)
    where
        H: EventHandler + 'static,
    {
        self.handlers.push(Arc::new(DynAdapter { inner: handler }));
    }

    /// Number of registered handlers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// `true` when no handlers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Dispatch `event` to every registered handler.
    ///
    /// Takes `event` by reference so the [`Runner`](crate::Runner) can
    /// emit the same `Event` to both the user-defined handler stream and
    /// the system observer stream (per rev17 §F3) without cloning. Each
    /// handler still sees a borrowed reference; the inner trait already
    /// expects `&Event`.
    ///
    /// `iteration` is the per-turn snapshot the runner builds before each
    /// emit. Handlers that render Handlebars templates against
    /// [`RenderContext`](crate::template::RenderContext) /
    /// [`LifecycleRenderContext`](crate::template::LifecycleRenderContext)
    /// pair the snapshot with the event's signal (when present) so
    /// `{{iteration.*}}` resolves consistently from `runner_starting`
    /// through `runner_finished`.
    ///
    /// Returns an [`EmitReport`] describing how many handlers returned
    /// `Err`. The report is cheap to ignore when the caller does not
    /// care about observability, but the [`Runner`](crate::Runner) uses
    /// it to populate
    /// [`RunnerSummary::event_handler_error_count`](crate::RunnerSummary::event_handler_error_count).
    pub async fn emit(&self, event: &Event, iteration: &IterationContext) -> EmitReport {
        let mut error_count = 0usize;
        for (idx, handler) in self.handlers.iter().enumerate() {
            if let Err(err) = handler.handle(event, iteration).await {
                error_count += 1;
                warn!(handler_index = idx, error = %err, "event handler returned error");
            }
        }
        EmitReport { error_count }
    }
}

impl std::fmt::Debug for EventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("handlers", &self.handlers.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::signal::{Metadata, Signal};

    fn sample_event() -> Event {
        Event::WorkspaceTeardownFinished {
            signal: Signal::new(Metadata::new()),
            path: PathBuf::from("/tmp/iter-test"),
        }
    }

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    struct CountingHandler {
        counter: Arc<AtomicUsize>,
    }

    impl EventHandler for CountingHandler {
        async fn handle(
            &self,
            _event: &Event,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingHandler;

    impl EventHandler for FailingHandler {
        async fn handle(
            &self,
            _event: &Event,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            Err("handler failed".into())
        }
    }

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

    #[tokio::test]
    async fn empty_emitter_is_a_noop() {
        let emitter = EventEmitter::new();
        let report = emitter.emit(&sample_event(), &iter_ctx()).await;
        assert!(report.is_clean());
        assert_eq!(report.error_count, 0);
    }

    #[tokio::test]
    async fn calls_every_registered_handler() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventEmitter::new();
        emitter.register(CountingHandler {
            counter: Arc::clone(&counter),
        });
        emitter.register(CountingHandler {
            counter: Arc::clone(&counter),
        });

        let report = emitter.emit(&sample_event(), &iter_ctx()).await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(report.error_count, 0);
    }

    #[tokio::test]
    async fn handler_error_does_not_abort_but_is_counted() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventEmitter::new();
        emitter.register(FailingHandler);
        emitter.register(CountingHandler {
            counter: Arc::clone(&counter),
        });

        let report = emitter.emit(&sample_event(), &iter_ctx()).await;

        // The counting handler must still have been called even though the
        // first handler returned an error.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // But the failing handler's error is visible in the report.
        assert_eq!(report.error_count, 1);
        assert!(!report.is_clean());
    }

    #[tokio::test]
    async fn multiple_failing_handlers_all_counted() {
        let mut emitter = EventEmitter::new();
        emitter.register(FailingHandler);
        emitter.register(FailingHandler);
        emitter.register(FailingHandler);

        let report = emitter.emit(&sample_event(), &iter_ctx()).await;

        assert_eq!(report.error_count, 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_emit_is_safe() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut emitter = EventEmitter::new();
        emitter.register(CapturingHandler {
            events: Arc::clone(&events),
        });
        let emitter = Arc::new(emitter);

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let emitter = Arc::clone(&emitter);
            tasks.push(tokio::spawn(async move {
                let _ = emitter.emit(&sample_event(), &iter_ctx()).await;
            }));
        }
        for t in tasks {
            t.await.expect("join");
        }

        assert_eq!(events.lock().unwrap().len(), 16);
    }
}
