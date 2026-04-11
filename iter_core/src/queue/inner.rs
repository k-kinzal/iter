//! [`Queue`] trait — the priority queue connecting signal sources and the
//! [`Runner`](crate::runner::Runner).
//!
//! Uses Return-Position-Impl-Trait-In-Trait (RPITIT) so that implementors
//! can write `async fn` bodies without paying for an extra allocation per
//! call. The associated futures are required to be `Send` so they can be
//! polled by the multi-threaded `tokio` runtime.

use std::future::Future;

use tokio_util::sync::CancellationToken;

use crate::queue::Priority;
use crate::signal::Signal;

/// A priority queue of [`Signal`]s.
///
/// Implementations are expected to be `Send + Sync` and cheap to clone via
/// `Arc`. The `dequeue` method must respect the supplied
/// [`CancellationToken`] and return `Ok(None)` once it observes cancellation
/// *or* once the queue has been [`close`](Self::close)d and drained.
///
/// # Delivery semantics
///
/// Signal delivery is **at-most-once across a process crash**: once
/// `dequeue` returns a signal to the runner, the backend considers it
/// delivered. Cloud backends with explicit ack (SQS, Service Bus, Pub/Sub,
/// Kafka) auto-ack on receipt to preserve this semantic — matching the
/// in-process behaviour of [`InMemoryQueue`](crate::queue::InMemoryQueue),
/// where a crash between `dequeue` and prompt completion drops the signal.
///
/// # Priority ordering
///
/// Priority ordering is **best-effort**. The in-process and Redis backends
/// guarantee strict highest-priority-first dequeuing with FIFO tie-breaking.
/// Streaming and pull-based cloud backends (Kafka, Kinesis, Pub/Sub
/// streaming-pull, SQS long-poll) deliver in their native FIFO/stream order
/// regardless of priority; the priority is preserved on the envelope as a
/// message attribute / payload field for observability and downstream
/// routing, but does not influence delivery order at the broker.
pub trait Queue: Send + Sync {
    /// Queue-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Push a signal onto the queue with the given priority.
    ///
    /// Implementations should reject signals queued after [`close`](Self::close)
    /// so callers cannot silently lose work; returning an error is preferable
    /// to dropping.
    fn queue(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Pop the next signal off the queue, blocking until one is available or
    /// the supplied [`CancellationToken`] is triggered.
    ///
    /// Returns `Ok(None)` when:
    ///
    /// * the cancellation token is triggered before a signal becomes
    ///   available, or
    /// * the queue has been [`close`](Self::close)d by its producer and
    ///   every previously-enqueued signal has already been handed out
    ///   (the "drained" terminal state).
    ///
    /// The runner distinguishes the two cases by observing the
    /// [`CancellationToken`]: drained queues return `Ok(None)` even though
    /// cancel is not set, which maps to
    /// [`RunnerTerminationReason::QueueDrained`](crate::RunnerTerminationReason::QueueDrained).
    fn dequeue(
        &self,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<Option<Signal>, Self::Error>> + Send;

    /// Mark the queue as closed.
    ///
    /// A closed queue rejects further `queue` calls and causes any
    /// currently-parked or subsequently-called `dequeue` to return
    /// `Ok(None)` once every already-enqueued signal has been handed out.
    /// This is how finite triggers (e.g. the `files` trigger after it
    /// consumes its list, or the `loop` trigger once it reaches
    /// `max_iteration`) let the runner exit cleanly without requiring an
    /// external SIGTERM.
    ///
    /// Implementations must be idempotent: calling `close` on an
    /// already-closed queue is a no-op and must not error. The default
    /// implementation is a no-op, which preserves the historical
    /// "always-on" behaviour for backends that cannot implement close
    /// semantics meaningfully (e.g. a shared Redis queue whose producer
    /// set extends beyond the current process).
    fn close(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
}
