//! [`EventDispatcher`] — routes [`HookEvent`]s to handlers registered for specific
//! [`EventName`]s.
//!
//! Because the [`EventAction`] trait uses RPITIT it is not directly
//! dyn-compatible. The emitter therefore wraps each handler in an internal
//! erased trait that returns a `Pin<Box<dyn Future>>` so a heterogeneous list
//! of handlers can be stored in a `Vec`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::warn;

use super::event::{EventName, HookEvent};
use super::event_handler::{BoxError, EventAction};
use super::iteration::IterationContext;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Internal dyn-compatible adapter for [`EventAction`].
trait DynEventAction: Send + Sync {
    fn handle<'a>(
        &'a self,
        event: &'a HookEvent,
        iteration: &'a IterationContext,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
}

struct DynAdapter<H: EventAction> {
    inner: H,
}

impl<H: EventAction> DynEventAction for DynAdapter<H> {
    fn handle<'a>(
        &'a self,
        event: &'a HookEvent,
        iteration: &'a IterationContext,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(self.inner.handle(event, iteration))
    }
}

/// Event dispatcher that routes [`HookEvent`]s to registered
/// [`EventAction`](crate::runner::EventAction)s by [`EventName`].
///
/// Actions are registered via [`EventDispatcher::on`] with an explicit
/// [`EventName`]. On [`EventDispatcher::emit`], only the actions registered
/// for the event's name are invoked, in registration order.
///
/// Failing handlers are logged via [`tracing`] at `warn` level and the
/// remaining handlers still run.
#[derive(Default, Clone)]
pub struct EventDispatcher {
    routes: HashMap<EventName, Vec<Arc<dyn DynEventAction>>>,
    handler_count: usize,
}

impl EventDispatcher {
    /// Build an emitter with no registered handlers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler for a specific [`EventName`].
    ///
    /// The handler will only be invoked when an event with the matching
    /// name is emitted. Handlers registered for the same name are invoked
    /// in registration order.
    pub fn on<H>(&mut self, name: EventName, handler: H)
    where
        H: EventAction + 'static,
    {
        self.routes
            .entry(name)
            .or_default()
            .push(Arc::new(DynAdapter { inner: handler }));
        self.handler_count += 1;
    }

    /// Number of registered handlers (across all event names).
    #[must_use]
    pub fn len(&self) -> usize {
        self.handler_count
    }

    /// `true` when no handlers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handler_count == 0
    }

    /// Dispatch `event` to handlers registered for its [`EventName`].
    ///
    /// Takes `event` by reference so the [`Runner`](crate::Runner) can
    /// emit the same `HookEvent` to both the user-defined handler stream and
    /// the system observer stream without cloning. Each handler still sees
    /// a borrowed reference; the inner trait already expects `&HookEvent`.
    ///
    /// `iteration` is the per-iteration snapshot the runner builds before each
    /// emit. Handlers that render Handlebars templates against
    /// [`IterationRenderContext`](crate::template::IterationRenderContext) /
    /// [`RunnerRenderContext`](crate::template::RunnerRenderContext)
    /// pair the snapshot with the event's signal (when present) so
    /// `{{iteration.*}}` resolves consistently from `runner_starting`
    /// through `runner_finished`.
    ///
    pub async fn emit(&self, event: &HookEvent, iteration: &IterationContext) {
        self.emit_counted(event, iteration).await;
    }

    pub(crate) async fn emit_counted(
        &self,
        event: &HookEvent,
        iteration: &IterationContext,
    ) -> usize {
        let name = event.name();
        let mut error_count = 0usize;
        if let Some(handlers) = self.routes.get(&name) {
            for (idx, handler) in handlers.iter().enumerate() {
                if let Err(err) = handler.handle(event, iteration).await {
                    error_count += 1;
                    warn!(
                        event_name = ?name,
                        handler_index = idx,
                        error = %err,
                        "event handler returned error",
                    );
                }
            }
        }
        error_count
    }
}

impl std::fmt::Debug for EventDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventDispatcher")
            .field("handlers", &self.handler_count)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::signal::{Metadata, Signal};

    fn sample_event() -> HookEvent {
        HookEvent::WorkspaceTeardownFinished {
            signal: Signal::new(Metadata::new()).into(),
            path: PathBuf::from("/tmp/iter-test"),
        }
    }

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    struct CountingHandler {
        counter: Arc<AtomicUsize>,
    }

    impl EventAction for CountingHandler {
        async fn handle(
            &self,
            _event: &HookEvent,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingHandler;

    impl EventAction for FailingHandler {
        async fn handle(
            &self,
            _event: &HookEvent,
            _iteration: &IterationContext,
        ) -> Result<(), BoxError> {
            Err("handler failed".into())
        }
    }

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

    #[tokio::test]
    async fn empty_emitter_is_a_noop() {
        let emitter = EventDispatcher::new();
        let error_count = emitter.emit_counted(&sample_event(), &iter_ctx()).await;
        assert_eq!(error_count, 0);
        assert_eq!(error_count, 0);
    }

    #[tokio::test]
    async fn calls_every_handler_for_matching_event() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventDispatcher::new();
        emitter.on(
            EventName::WorkspaceTeardownFinished,
            CountingHandler {
                counter: Arc::clone(&counter),
            },
        );
        emitter.on(
            EventName::WorkspaceTeardownFinished,
            CountingHandler {
                counter: Arc::clone(&counter),
            },
        );

        let error_count = emitter.emit_counted(&sample_event(), &iter_ctx()).await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(error_count, 0);
    }

    #[tokio::test]
    async fn does_not_invoke_handlers_for_other_events() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventDispatcher::new();
        emitter.on(
            EventName::RunnerStarting,
            CountingHandler {
                counter: Arc::clone(&counter),
            },
        );

        let error_count = emitter.emit_counted(&sample_event(), &iter_ctx()).await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(error_count, 0);
    }

    #[tokio::test]
    async fn handler_error_does_not_abort_but_is_counted() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventDispatcher::new();
        emitter.on(EventName::WorkspaceTeardownFinished, FailingHandler);
        emitter.on(
            EventName::WorkspaceTeardownFinished,
            CountingHandler {
                counter: Arc::clone(&counter),
            },
        );

        let error_count = emitter.emit_counted(&sample_event(), &iter_ctx()).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(error_count, 1);
        assert!(error_count > 0);
    }

    #[tokio::test]
    async fn multiple_failing_handlers_all_counted() {
        let mut emitter = EventDispatcher::new();
        emitter.on(EventName::WorkspaceTeardownFinished, FailingHandler);
        emitter.on(EventName::WorkspaceTeardownFinished, FailingHandler);
        emitter.on(EventName::WorkspaceTeardownFinished, FailingHandler);

        let error_count = emitter.emit_counted(&sample_event(), &iter_ctx()).await;

        assert_eq!(error_count, 3);
    }

    #[tokio::test]
    async fn error_events_route_to_runner_error_name() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut emitter = EventDispatcher::new();
        emitter.on(
            EventName::RunnerError,
            CountingHandler {
                counter: Arc::clone(&counter),
            },
        );

        let error_event = HookEvent::AgentRunFailed {
            signal_id: Signal::new(Metadata::new()).id(),
            error: "boom".into(),
        };
        emitter.emit(&error_event, &iter_ctx()).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_emit_is_safe() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut emitter = EventDispatcher::new();
        emitter.on(
            EventName::WorkspaceTeardownFinished,
            CapturingHandler {
                events: Arc::clone(&events),
            },
        );
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
