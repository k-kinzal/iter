//! [`QueueError`] — the erased error every [`Queue`](crate::queue::Queue)
//! trait method returns.
//!
//! The [`Queue`](crate::queue::Queue) trait is dyn-compatible, so it names no
//! associated error type — `dyn Queue` would have nowhere to put one. Each
//! backend's concrete error (`FileQueueError`, `ShellQueueError`, …) is erased
//! behind this boxed source. Callers that need the concrete error back can
//! recover it with [`QueueError::downcast_ref`].

use std::error::Error as StdError;
use std::fmt;

/// Error returned by [`Queue`](crate::queue::Queue) trait methods.
///
/// Wraps whichever backend's concrete error occurred so the trait stays
/// dyn-compatible. Build one from any backend error with [`QueueError::new`].
pub struct QueueError(Box<dyn StdError + Send + Sync + 'static>);

impl QueueError {
    /// Erase a concrete backend error into a [`QueueError`].
    #[must_use]
    pub fn new<E: StdError + Send + Sync + 'static>(source: E) -> Self {
        Self(Box::new(source))
    }

    /// Recover the concrete backend error, if it is of type `E`.
    #[must_use]
    pub fn downcast_ref<E: StdError + 'static>(&self) -> Option<&E> {
        self.0.downcast_ref::<E>()
    }
}

impl fmt::Debug for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl StdError for QueueError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        // Transparent erasure: `Display` already forwards to the inner error,
        // so the chain continues from the inner's *own* source rather than
        // re-reporting the inner error itself (which would print its message
        // twice in `{:#}`-style chain walks).
        self.0.source()
    }
}
