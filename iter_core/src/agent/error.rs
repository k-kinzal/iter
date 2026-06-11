//! [`AgentError`] — iter's domain error for a single agent run.
//!
//! This is the error half of the **Agent level** (see [`AgentRun`]). It is
//! deliberately minimal: it enumerates only the failure classes iter
//! actually consumes — either by branching on the variant (today only the
//! [`AgentRouter`] matches [`AgentError::TokenLimit`]) or by reading it into
//! a Factor (the runner reads the exit code off [`AgentError::Failed`] for
//! the `iteration.previous_exit_code` template field).
//!
//! The rich, CLI-shaped error hierarchy — auth/quota/rate/context/network
//! classes, HTTP status codes, retry flags — lives at the **Command level**
//! (`drivers/<cli>/command.rs`). Each driver, acting as an Adapter, projects
//! that hierarchy down to one of these variants. Do **not** grow this enum
//! into an enumerate-so-the-runner-can-branch vocabulary: the runner does
//! not branch on the class, and an unconsumed variant is dead surface.
//!
//! [`AgentRun`]: crate::agent::AgentRun
//! [`AgentRouter`]: crate::agent::AgentRouter

use std::io;

use thiserror::Error;

/// Errors produced while driving a CLI-backed agent for one run.
///
/// All drivers share this single enum; richness that varies per CLI is kept
/// in the per-CLI Command error and collapsed here at the Adapter boundary.
#[derive(Debug, Error)]
pub enum AgentError {
    /// The agent was asked to shut down via its [`CancellationToken`] before
    /// the child process finished. The underlying process has been sent a
    /// kill signal; its exit state is not propagated.
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    #[error("agent run was cancelled")]
    Cancelled,

    /// The iteration exceeded the configured timeout. The agent was given a
    /// grace period to complete its shutdown (via its cancellation token)
    /// before this error was returned.
    #[error("iteration exceeded timeout of {0:?}")]
    IterationTimeout(std::time::Duration),

    /// The agent hit the model's context-window or token limit. The
    /// contained string is an informational excerpt around the detected
    /// pattern — not machine-parseable. The router matches on the *variant*
    /// (not the payload) to trigger fallback to the next agent.
    #[error("agent token limit exceeded: {0}")]
    TokenLimit(String),

    /// The agent could not be launched or was misconfigured: an empty
    /// command, an I/O error while spawning or talking to the child, a hook
    /// bundle setup failure, an argument-parse rejection, or a fatal startup
    /// exit (e.g. Gemini's `41`–`58`). The agent never ran.
    #[error("agent failed to launch: {0}")]
    Launch(String),

    /// The agent process was terminated by the indicated signal number
    /// (abnormal, process-level termination — distinct from an in-band
    /// agent failure).
    #[error("agent terminated by signal {0}")]
    TerminatedBySignal(i32),

    /// The agent ran but reported a failure: a non-zero process exit, or an
    /// in-band failure event in the output (the exit-0-but-failed CLIs).
    /// `code` is the process exit code when one was produced — `None` when
    /// the failure was detected from the output of a process that exited
    /// `0`. The message is a short, human-readable summary projected from
    /// the Command error.
    #[error("agent failed{}: {message}", match .code { Some(c) => format!(" with exit code {c}"), None => String::new() })]
    Failed {
        /// Process exit code, when the failure came with one.
        code: Option<i32>,
        /// Human-readable summary projected from the Command error.
        message: String,
    },
}

impl AgentError {
    /// Short, stable label for logs / lifecycle records. Mirrors the runner's
    /// `iter.agent.result` span attribute vocabulary.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::IterationTimeout(_) => "timeout",
            Self::TokenLimit(_) => "token_limit",
            Self::Launch(_) => "errored",
            Self::TerminatedBySignal(_) => "terminated_by_signal",
            Self::Failed { .. } => "failure",
        }
    }

    /// Process exit code associated with this error, when one is meaningful.
    /// `None` for cancellation, timeout, token-limit, launch failure, and
    /// signal termination (a signal is not an exit code).
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Failed { code, .. } => *code,
            _ => None,
        }
    }
}

impl From<io::Error> for AgentError {
    /// An I/O error spawning the child or touching its streams / state files
    /// means the agent could not be driven — collapse it into [`Launch`].
    ///
    /// [`Launch`]: AgentError::Launch
    fn from(err: io::Error) -> Self {
        Self::Launch(err.to_string())
    }
}
