//! How a Process Runtime captures the Agent's stdout/stderr.

use std::path::{Path, PathBuf};

/// How a Process Runtime captures the Agent's stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputPolicy {
    /// Every captured process: write to `<log_dir>/log.ndjson` only.
    LogOnly {
        /// Directory holding `log.ndjson`.
        log_dir: PathBuf,
    },
    /// Interactive agents that inherit the TTY directly. The Process
    /// Runtime captures nothing.
    Passthrough,
}

impl OutputPolicy {
    /// Borrow the log directory, when one is owned by this policy.
    #[must_use]
    pub(crate) fn log_dir(&self) -> Option<&Path> {
        match self {
            Self::LogOnly { log_dir } => Some(log_dir),
            Self::Passthrough => None,
        }
    }

    /// `true` when this policy writes anything to disk.
    #[must_use]
    pub(crate) fn writes_log_files(&self) -> bool {
        matches!(self, Self::LogOnly { .. })
    }
}
