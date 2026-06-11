//! `RunnerObserver` — the system-contract sink for
//! [`RunnerLifecycleEvent`](super::RunnerLifecycleEvent).
//!
//! The Runner emits two parallel output streams:
//!
//! 1. The user-defined [`HookEvent`](super::HookEvent) stream, which feeds
//!    declared `on …` hooks. Failure of a hook is the user's problem.
//! 2. The system [`RunnerLifecycleEvent`](super::RunnerLifecycleEvent) stream, which
//!    feeds process-runtime observers.
//!
//! Stream 2 is consumed via this module's [`RunnerObserver`] trait. The
//! Runner stores `Arc<dyn DynRunnerObserver>` so observers can be plugged
//! in dynamically.
//!
//! The canonical concrete implementation is
//! [`LifecycleObserver`](crate::process::observer::LifecycleObserver),
//! which lives in the process module because it is a process-runtime
//! consumer — it re-emits lifecycle records as tracing events routed
//! into `log.ndjson`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::BoxFuture;

use super::lifecycle::RunnerLifecycleEvent;
use crate::runner::BoxError;

/// Async observer of the Runner's lifecycle stream.
///
/// Implementations are stored as `Arc<dyn DynRunnerObserver>` by the
/// Runner. To make adapter writing painless, this trait uses RPITIT
/// (return-position `impl Trait`); use the [`DynRunnerObserver`]
/// companion to obtain a dyn-safe form.
pub trait RunnerObserver: Send + Sync {
    /// Observe a single lifecycle event.
    fn observe<'a>(
        &'a self,
        lifecycle: &'a RunnerLifecycleEvent,
    ) -> impl Future<Output = Result<(), BoxError>> + Send + 'a;
}

/// Object-safe companion to [`RunnerObserver`].
///
/// `Arc<dyn DynRunnerObserver>` is what the Runner actually stores.
/// Concrete `RunnerObserver` impls are upcast through the blanket
/// adapter below.
pub trait DynRunnerObserver: Send + Sync {
    /// Object-safe `observe`. Mirrors [`RunnerObserver::observe`] but
    /// returns a [`BoxFuture`].
    fn observe<'a>(
        &'a self,
        lifecycle: &'a RunnerLifecycleEvent,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
}

/// Blanket adapter making every [`RunnerObserver`] usable via
/// `Arc<dyn DynRunnerObserver>`.
impl<O> DynRunnerObserver for O
where
    O: RunnerObserver + ?Sized,
{
    fn observe<'a>(
        &'a self,
        lifecycle: &'a RunnerLifecycleEvent,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(RunnerObserver::observe(self, lifecycle))
    }
}

/// Forwarding impl so an existing `Arc<T: RunnerObserver>` (e.g. the one
/// owned by [`crate::process::ProcessRuntime`]) can be handed straight to
/// [`RunnerBuilder::observer`](crate::RunnerBuilder::observer) without
/// re-wrapping.
impl<T> RunnerObserver for Arc<T>
where
    T: RunnerObserver + ?Sized,
{
    fn observe<'a>(
        &'a self,
        lifecycle: &'a RunnerLifecycleEvent,
    ) -> impl Future<Output = Result<(), BoxError>> + Send + 'a {
        T::observe(self, lifecycle)
    }
}

/// Boxed-future variant of [`RunnerObserver::observe`] for sites that
/// need an explicit `BoxFuture` (e.g. selecting across heterogeneous
/// observers).
pub type ObserveFuture<'a> = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'a>>;
