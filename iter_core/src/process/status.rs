//! Lifecycle status of a process record and the *only* legal transitions
//! between states.
//!
//! # Invariants (rev10/rev13/rev17)
//!
//! - [`ProcessStatus`] is the canonical 5-state lifecycle: `Initializing →
//!   Running → {Stopped|Failed|Killed}`.
//! - On disk the value is serialised as `snake_case` (`initializing`,
//!   `running`, `stopped`, `failed`, `killed`). Other forms are rejected by
//!   [`read_status`] with a [`CorruptStatusError`].
//! - [`is_allowed`] is a *generic* transition table: it **never** accepts
//!   `Initializing → Running`. Running can only be reached through the in-place
//!   writer used by `locked_initial_write` / `locked_adoption_write` inside
//!   `process::status_file`. This shuts the door on the "pid file not yet
//!   written but status already says Running" hazard at the type level.
//!
//! Diagram:
//!
//! ```text
//!                   ┌──────────────┐
//!                   │ Initializing │
//!                   └──────┬───────┘
//!                          │  (only inside locked_initial_write /
//!                          │   locked_adoption_write — NEVER via
//!                          │   the generic `transition` API)
//!                          ▼
//!                       Running
//!                       /  |  \
//!                      /   |   \
//!                     ▼    ▼    ▼
//!                Stopped Failed Killed   (terminal, no further moves)
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Lifecycle status as it appears in `~/.iter/proc/<id>/status`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    /// Created on disk; the runtime has not yet completed bootstrap.
    Initializing,
    /// Bootstrap done, runner is executing.
    Running,
    /// Terminal: runner returned `Ok`.
    Stopped,
    /// Terminal: runner returned `Err` or panicked.
    Failed,
    /// Terminal: runner was terminated by a signal (SIGINT/SIGTERM/SIGKILL via
    /// `iter stop`/`iter kill`).
    Killed,
}

impl ProcessStatus {
    /// Canonical on-disk token (`snake_case`).
    #[must_use]
    pub fn as_serde_str(self) -> &'static str {
        match self {
            ProcessStatus::Initializing => "initializing",
            ProcessStatus::Running => "running",
            ProcessStatus::Stopped => "stopped",
            ProcessStatus::Failed => "failed",
            ProcessStatus::Killed => "killed",
        }
    }

    /// True when the status is one of the three terminal values.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ProcessStatus::Stopped | ProcessStatus::Failed | ProcessStatus::Killed
        )
    }
}

impl fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_serde_str())
    }
}

impl FromStr for ProcessStatus {
    type Err = UnknownStatusToken;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "initializing" => Ok(ProcessStatus::Initializing),
            "running" => Ok(ProcessStatus::Running),
            "stopped" => Ok(ProcessStatus::Stopped),
            "failed" => Ok(ProcessStatus::Failed),
            "killed" => Ok(ProcessStatus::Killed),
            other => Err(UnknownStatusToken(other.to_owned())),
        }
    }
}

/// Returned from [`FromStr`] when the on-disk token is not one of the five
/// canonical names. The contained `String` carries the offending token so
/// upper layers can include it in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownStatusToken(pub String);

impl fmt::Display for UnknownStatusToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown status token: {:?}", self.0)
    }
}

impl std::error::Error for UnknownStatusToken {}

/// Result of a successful `transition` call.
///
/// Always carries both endpoints so callers can log and observe the change
/// without ambiguity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TransitionResult {
    /// Status observed before the write.
    pub from: ProcessStatus,
    /// Status committed to disk.
    pub to: ProcessStatus,
}

/// Returns `true` only for the transitions reachable via the generic
/// `transition` API (`process::status_file::transition`).
///
/// Crucially, **`Initializing → Running` is rejected here.** Reaching `Running`
/// requires writing the pid file in the *same* flock-protected critical
/// section, which is the responsibility of `locked_initial_write` /
/// `locked_adoption_write`. Both call the private `write_status_in_place`
/// helper directly, never going through `transition`.
///
/// | from \ to     | Running | Stopped | Failed | Killed |
/// |---------------|---------|---------|--------|--------|
/// | Initializing  | NO      | NO      | YES    | YES    |
/// | Running       | NO      | YES     | YES    | YES    |
/// | terminal      | NO      | NO      | NO     | NO     |
#[must_use]
pub fn is_allowed(from: ProcessStatus, to: ProcessStatus) -> bool {
    use ProcessStatus::{Failed, Initializing, Killed, Running, Stopped};

    // Identity moves are not transitions.
    if from == to {
        return false;
    }
    // Running can never be re-entered through the generic API.
    if to == Running {
        return false;
    }
    matches!(
        (from, to),
        (Initializing, Failed | Killed) | (Running, Stopped | Failed | Killed)
    )
}

/// Error returned when `read_status` cannot decode the current contents of
/// the status file.
///
/// `read_status` is canonically the entry point at the top of
/// [`reconcile_under_lock`]; carrying the raw bytes lets `Diagnostic` surface
/// the corruption without re-reading the file.
///
/// [`reconcile_under_lock`]: crate::process::status_file::reconcile_under_lock
#[derive(Debug)]
pub struct CorruptStatusError {
    /// What was wrong with the contents.
    pub kind: CorruptStatusKind,
    /// Verbatim file contents at the moment of failure (truncated to a
    /// bounded length by the reader). Useful for diagnostics; not for
    /// security-sensitive decisions.
    pub raw_bytes: Vec<u8>,
}

impl fmt::Display for CorruptStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "corrupt status file ({:?})", self.kind)
    }
}

impl std::error::Error for CorruptStatusError {}

/// Why `read_status` rejected the file contents. See `CorruptStatusError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorruptStatusKind {
    /// File was zero bytes (or only whitespace). Typically observed when a
    /// previous `set_len(0) + write_all(..)` was interrupted between the
    /// truncate and the write.
    EmptyBody,
    /// Read returned fewer bytes than expected before EOF.
    TruncatedRead {
        /// Number of bytes actually returned.
        read: usize,
    },
    /// Body was non-empty but not one of the canonical tokens.
    UnknownToken(String),
    /// The canonical token was present but extra bytes followed it.
    TrailingGarbage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_round_trip() {
        for status in [
            ProcessStatus::Initializing,
            ProcessStatus::Running,
            ProcessStatus::Stopped,
            ProcessStatus::Failed,
            ProcessStatus::Killed,
        ] {
            let token = status.as_serde_str();
            let parsed: ProcessStatus = token.parse().expect("known token");
            assert_eq!(status, parsed);
            // serde_json should produce the same token (`"initializing"` etc.).
            let json = serde_json::to_string(&status).expect("serialize");
            assert_eq!(json, format!("\"{token}\""));
            let de: ProcessStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(status, de);
        }
    }

    #[test]
    fn unknown_token_rejected() {
        let err: Result<ProcessStatus, _> = "BOOTING".parse();
        assert!(err.is_err());
    }

    #[test]
    fn is_allowed_table_matches_spec() {
        use ProcessStatus::{Failed, Initializing, Killed, Running, Stopped};

        // Initializing row: only Failed/Killed are allowed.
        assert!(!is_allowed(Initializing, Running)); // ← THE invariant
        assert!(!is_allowed(Initializing, Stopped));
        assert!(is_allowed(Initializing, Failed));
        assert!(is_allowed(Initializing, Killed));

        // Running row: Stopped/Failed/Killed allowed.
        assert!(!is_allowed(Running, Initializing));
        assert!(!is_allowed(Running, Running));
        assert!(is_allowed(Running, Stopped));
        assert!(is_allowed(Running, Failed));
        assert!(is_allowed(Running, Killed));

        // Terminal rows: nothing allowed.
        for from in [Stopped, Failed, Killed] {
            for to in [Initializing, Running, Stopped, Failed, Killed] {
                assert!(!is_allowed(from, to), "{from:?} → {to:?} should be NO");
            }
        }
    }

    #[test]
    fn running_column_is_all_no() {
        // The whole point of B1.1 (rev10).
        for from in [
            ProcessStatus::Initializing,
            ProcessStatus::Running,
            ProcessStatus::Stopped,
            ProcessStatus::Failed,
            ProcessStatus::Killed,
        ] {
            assert!(!is_allowed(from, ProcessStatus::Running));
        }
    }

    #[test]
    fn is_terminal_marks_only_terminal_states() {
        assert!(!ProcessStatus::Initializing.is_terminal());
        assert!(!ProcessStatus::Running.is_terminal());
        assert!(ProcessStatus::Stopped.is_terminal());
        assert!(ProcessStatus::Failed.is_terminal());
        assert!(ProcessStatus::Killed.is_terminal());
    }
}
