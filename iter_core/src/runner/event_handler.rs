//! [`EventHandler`] trait — sinks for the [`Event`](crate::runner::Event)
//! stream emitted by the runner.

use std::future::Future;

use crate::runner::{Event, IterationContext};

/// A boxed, type-erased error returned by [`EventHandler`] implementations.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// An asynchronous sink for [`Event`]s.
///
/// Implementations should be cheap to clone or hold by reference. The
/// [`EventEmitter`](crate::runner::EventEmitter) calls `handle` on every
/// registered handler in turn and logs (but does not propagate) any error.
///
/// `iteration` carries the runner's per-turn state snapshot — the same
/// view templates see as `{{iteration.*}}`. Implementations that render
/// templates against a [`Signal`](crate::Signal) should pair it with this
/// snapshot via
/// [`RenderContext`](crate::template::RenderContext); signal-less
/// lifecycle events should use
/// [`LifecycleRenderContext`](crate::template::LifecycleRenderContext).
pub trait EventHandler: Send + Sync {
    /// Process a single event paired with the current iteration snapshot.
    fn handle(
        &self,
        event: &Event,
        iteration: &IterationContext,
    ) -> impl Future<Output = Result<(), BoxError>> + Send;
}
