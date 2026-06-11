//! Cross-backend [`RetryPolicy`].
//!
//! Used by the SQS backend's SDK retry surface. Each backend picks the subset
//! of [`RetryMode`] values it understands and reports a clear error for
//! unsupported choices in its lowerer.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How a backoff schedule is computed across retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryMode {
    /// SDK-default schedule. The exact behaviour is per-SDK.
    Standard,
    /// AWS-specific adaptive throttling that adjusts to client-side
    /// throttling signals.
    Adaptive,
    /// Constant `initial_backoff` between every attempt.
    Fixed,
    /// Exponential backoff starting from `initial_backoff`, capped at
    /// `max_backoff`.
    Exponential,
}

impl Default for RetryMode {
    fn default() -> Self {
        Self::Standard
    }
}

/// Cross-backend retry policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Backoff strategy. See [`RetryMode`].
    pub mode: RetryMode,
    /// Total attempt count including the first try. Defaults to `3`.
    pub max_attempts: u32,
    /// Initial wait between attempts. Defaults to `200ms`.
    pub initial_backoff: Duration,
    /// Cap on the per-attempt wait. Defaults to `30s`.
    pub max_backoff: Duration,
    /// Optional per-attempt timeout.
    pub try_timeout: Option<Duration>,
    /// Pub/Sub-only: gRPC status codes that should be retried.
    pub retryable_codes: Option<Vec<String>>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            mode: RetryMode::default(),
            max_attempts: 3,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(30),
            try_timeout: None,
            retryable_codes: None,
        }
    }
}

impl RetryPolicy {
    /// Compute the wait between attempts `attempt` and `attempt + 1`,
    /// honouring [`RetryPolicy::max_backoff`].
    ///
    /// `attempt` is 1-based: the wait *before* the second attempt is
    /// `delay_for_attempt(1)`.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        match self.mode {
            RetryMode::Standard | RetryMode::Adaptive | RetryMode::Exponential => {
                let base = self.initial_backoff;
                let factor = 1u32
                    .checked_shl(attempt.saturating_sub(1))
                    .unwrap_or(u32::MAX);
                let next = base.saturating_mul(factor);
                next.min(self.max_backoff)
            }
            RetryMode::Fixed => self.initial_backoff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_three_standard_attempts() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.mode, RetryMode::Standard);
    }

    #[test]
    fn exponential_doubles_until_capped() {
        let p = RetryPolicy {
            mode: RetryMode::Exponential,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(500),
            ..RetryPolicy::default()
        };
        assert_eq!(p.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(p.delay_for_attempt(2), Duration::from_millis(200));
        assert_eq!(p.delay_for_attempt(3), Duration::from_millis(400));
        // 800ms would exceed cap, clamped to 500ms.
        assert_eq!(p.delay_for_attempt(4), Duration::from_millis(500));
    }

    #[test]
    fn fixed_returns_initial_every_attempt() {
        let p = RetryPolicy {
            mode: RetryMode::Fixed,
            initial_backoff: Duration::from_millis(150),
            ..RetryPolicy::default()
        };
        assert_eq!(p.delay_for_attempt(1), Duration::from_millis(150));
        assert_eq!(p.delay_for_attempt(7), Duration::from_millis(150));
    }
}
