//! Errors produced by [`ShellQueue`](super::ShellQueue).

use std::time::Duration;

use thiserror::Error;

/// Errors produced by [`ShellQueue`](super::ShellQueue).
#[derive(Debug, Error)]
pub enum ShellQueueError {
    /// I/O error spawning, communicating with, or reaping a child process.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// `enqueue` script exited non-zero.
    #[error("enqueue script exited with status {status}: {stderr}")]
    EnqueueFailed {
        /// Numeric exit status reported by the OS, or `-1` if the child was
        /// killed by a signal or timed out.
        status: i32,
        /// Captured stderr (truncated to a reasonable size by the OS).
        stderr: String,
    },

    /// `enqueue` script ran longer than the configured timeout.
    #[error("enqueue script timed out after {0:?}")]
    EnqueueTimeout(Duration),

    /// Caller asked to enqueue after the queue had been closed.
    #[error("queue is closed")]
    Closed,

    /// The interpreter string was empty after splitting on whitespace.
    #[error("interpreter must contain at least a program name")]
    EmptyInterpreter,
}
