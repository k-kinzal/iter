//! [`WorkspaceError`] — the erased error every [`Workspace`](crate::workspace::Workspace)
//! trait method returns.
//!
//! The [`Workspace`](crate::workspace::Workspace) trait is dyn-compatible, so
//! it names no associated error type — `dyn Workspace` would have nowhere to
//! put one. Each implementation's concrete error (`LocalWorkspaceError`,
//! `CloneWorkspaceError`, `SandboxWorkspaceError`, …) is erased behind this
//! boxed source. Callers that need the concrete error back can recover it with
//! [`WorkspaceError::downcast_ref`].
//!
//! This mirrors [`QueueError`](crate::queue::QueueError): one abstraction
//! style per axis — a closed enum at the definition layer, a trait object at
//! run time. The dispatch cost is irrelevant here: every `setup`/`teardown`
//! does filesystem work (or spawns a sandbox) that dominates an indirect call
//! by orders of magnitude.

use std::error::Error as StdError;
use std::fmt;

/// Error returned by [`Workspace`](crate::workspace::Workspace) trait methods.
///
/// Wraps whichever implementation's concrete error occurred so the trait
/// stays dyn-compatible. Build one from any concrete error with
/// [`WorkspaceError::new`].
pub struct WorkspaceError(Box<dyn StdError + Send + Sync + 'static>);

impl WorkspaceError {
    /// Erase a concrete workspace error into a [`WorkspaceError`].
    #[must_use]
    pub fn new<E: StdError + Send + Sync + 'static>(source: E) -> Self {
        Self(Box::new(source))
    }

    /// Recover the concrete workspace error, if it is of type `E`.
    #[must_use]
    pub fn downcast_ref<E: StdError + 'static>(&self) -> Option<&E> {
        self.0.downcast_ref::<E>()
    }
}

impl fmt::Debug for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl StdError for WorkspaceError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        // Transparent erasure: `Display` already forwards to the inner error,
        // so the chain continues from the inner's *own* source rather than
        // re-reporting the inner error itself.
        self.0.source()
    }
}
