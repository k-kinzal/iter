//! Operator-side shutdown intent for an iter process record.
//!
//! The generic OS-signal listener lives in [`iter_core::os_signal`]. This
//! module keeps only the CLI vocabulary: a shared cancellation token that
//! tells this process record's runner to stop.

use tokio_util::sync::CancellationToken;

/// The recorded intent to shut down: a shared [`CancellationToken`] and
/// nothing else.
///
/// Cloning is cheap (`CancellationToken` internals) and every clone shares
/// the same token. `ShutdownIntent` carries no termination taxonomy —
/// classifying why a run ended is the run record's concern and lives with
/// `process_lifecycle`, which layers a reason slot on top of this intent via
/// [`iter_core::os_signal::spawn_interrupt_listener`].
#[derive(Debug, Clone)]
pub(crate) struct ShutdownIntent {
    cancel: CancellationToken,
}

impl ShutdownIntent {
    /// Create a fresh intent backed by a brand-new [`CancellationToken`].
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// Build an intent around a pre-existing [`CancellationToken`].
    #[must_use]
    pub(crate) fn with_token(cancel: CancellationToken) -> Self {
        Self { cancel }
    }

    /// Borrow the underlying [`CancellationToken`].
    #[must_use]
    pub(crate) fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Trigger shutdown. Idempotent.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// `true` iff the underlying [`CancellationToken`] has fired.
    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Default for ShutdownIntent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_intent_starts_uncancelled() {
        let intent = ShutdownIntent::new();
        assert!(!intent.is_cancelled());
    }

    #[test]
    fn cancel_fires_token_and_is_idempotent() {
        let intent = ShutdownIntent::new();
        intent.cancel();
        intent.cancel();
        assert!(intent.is_cancelled());
        assert!(intent.token().is_cancelled());
    }

    #[test]
    fn clones_share_the_token() {
        let a = ShutdownIntent::new();
        let b = a.clone();
        b.cancel();
        assert!(a.is_cancelled());
    }

    #[test]
    fn with_token_shares_the_callers_token() {
        let token = CancellationToken::new();
        let intent = ShutdownIntent::with_token(token.clone());
        token.cancel();
        assert!(intent.is_cancelled());
    }
}
