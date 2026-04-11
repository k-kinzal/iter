//! [`AgentReport`] and related [`ExitStatus`] type.

use serde::{Deserialize, Serialize};

/// The exit status produced by an [`Agent`](crate::agent::Agent) process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExitStatus {
    /// Agent exited with code zero.
    Success,
    /// Agent exited with the provided non-zero exit code.
    Failure(i32),
    /// Agent was terminated by the indicated signal number.
    Signal(i32),
    /// Status could not be determined.
    Unknown,
}

impl ExitStatus {
    /// `true` only when the status is [`ExitStatus::Success`].
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Bookkeeping returned from [`Agent::run`](crate::agent::Agent::run).
///
/// [`AgentReport::last_output`] is surfaced through
/// [`Event::AgentFinished`](crate::runner::Event::AgentFinished) so that
/// event handlers (logging, debug UIs, and so on) can see the tail of what
/// the agent printed. It is informational only — the Runner does **not**
/// inspect agent output to decide whether to terminate. Termination is
/// Signal-driven: the loop stops when the queue drains, the cancel token
/// fires, or `--once` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReport {
    /// Process exit status.
    pub exit_status: ExitStatus,
    /// Tail of the agent's combined stdout/stderr, if any.
    pub last_output: Option<String>,
    /// Number of "turns" the agent took, when known.
    pub turn_count: Option<u32>,
}

impl AgentReport {
    /// Convenience constructor for a successful run with no extra data.
    #[must_use]
    pub fn success() -> Self {
        Self {
            exit_status: ExitStatus::Success,
            last_output: None,
            turn_count: None,
        }
    }
}
