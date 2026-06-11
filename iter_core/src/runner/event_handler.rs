//! [`EventAction`] trait — sinks for the [`HookEvent`](crate::runner::HookEvent)
//! stream emitted by the runner.

use std::future::Future;

use crate::runner::{HookEvent, IterationContext};

/// A boxed, type-erased error returned by [`EventAction`] implementations.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// An asynchronous sink for [`HookEvent`]s.
///
/// Implementations should be cheap to clone or hold by reference. The
/// [`EventDispatcher`](crate::runner::EventDispatcher) calls `handle` on each
/// registered action in registration order and logs (but does not propagate)
/// any error.
///
/// `iteration` carries the runner's per-iteration state snapshot — the same
/// view templates see as `{{iteration.*}}`. Implementations that render
/// templates against a [`Signal`](crate::Signal) should pair it with this
/// snapshot via
/// [`IterationRenderContext`](crate::template::IterationRenderContext); signal-less
/// lifecycle events should use
/// [`RunnerRenderContext`](crate::template::RunnerRenderContext).
pub trait EventAction: Send + Sync {
    /// Process a single event paired with the current iteration snapshot.
    fn handle(
        &self,
        event: &HookEvent,
        iteration: &IterationContext,
    ) -> impl Future<Output = Result<(), BoxError>> + Send;
}
