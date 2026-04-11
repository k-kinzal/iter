//! Error type shared by all [`crate::Agent`] implementations in this module.

use std::io;

use thiserror::Error;

/// Errors produced while configuring or driving a CLI-backed agent process.
///
/// All eight agent implementations in this crate share a single error enum to
/// keep the public surface small. Variants that are only emitted by a subset
/// of agents are still centralized here so callers can pattern-match without
/// juggling per-agent error types.
#[derive(Debug, Error)]
pub enum AgentError {
    /// An underlying I/O error while spawning the process, reading/writing
    /// standard streams, or touching hook settings files.
    #[error("agent I/O error: {0}")]
    Io(#[from] io::Error),

    /// The agent was invoked with an empty command vector. This is only
    /// reachable via [`GenericAgent`](crate::agent::GenericAgent) or an explicit
    /// misconfiguration of one of the concrete agents.
    #[error("agent command is empty")]
    EmptyCommand,

    /// Installing or restoring the project-local Claude hook bundle failed.
    /// The payload describes the specific failure. Only produced by
    /// [`ClaudeAgent`](crate::agent::ClaudeAgent) in interactive mode.
    #[error("hook setup failed: {0}")]
    HookSetup(String),

    /// Parsing the hook-written session state file failed. Only produced by
    /// [`ClaudeAgent`](crate::agent::ClaudeAgent) in interactive mode when the JSON
    /// written by the Stop hook script is malformed.
    #[error("hook state parse failed: {0}")]
    HookStateParse(String),

    /// The child process reported an exit code we could not interpret. This
    /// is distinct from a genuine non-zero failure: it means the platform did
    /// not expose *either* an exit code or a terminating signal (which, in
    /// practice, only happens on exotic POSIX edge cases).
    #[error("agent exited with unknown status")]
    UnknownExit,

    /// The agent was asked to shut down via its [`CancellationToken`]
    /// before the child process finished. The underlying process has been
    /// sent a kill signal; its exit state is not propagated.
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    #[error("agent run was cancelled")]
    Cancelled,

    /// The iteration exceeded the configured timeout. The agent was given
    /// a grace period to complete its shutdown (via its cancellation token)
    /// before this error was returned.
    #[error("iteration exceeded timeout of {0:?}")]
    IterationTimeout(std::time::Duration),
}
