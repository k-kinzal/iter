//! [`AgentError`] — iter's domain error for a single agent run.
//!
//! This is the error half of the **Agent level** (see [`AgentRun`]). It is
//! deliberately minimal: it enumerates only the failure classes iter
//! actually consumes — either by classifying router fallback eligibility or
//! by reading it into a Factor (the runner reads the exit code off
//! [`AgentError::Failed`] for the `iteration.previous_exit_code` template
//! field).
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

/// Failure classes the router can be configured to fall back on.
///
/// [`AgentError::Cancelled`] has no class: cancellation is cooperative
/// shutdown, never a fallback trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FallbackClass {
    /// [`AgentError::IterationTimeout`].
    Timeout,
    /// [`AgentError::TokenLimit`].
    TokenLimit,
    /// [`AgentError::Launch`].
    Launch,
    /// [`AgentError::TerminatedBySignal`].
    TerminatedBySignal,
    /// [`AgentError::Failed`].
    Failure,
}

impl FallbackClass {
    /// Stable token shared with [`AgentError::label`].
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::TokenLimit => "token_limit",
            Self::Launch => "errored",
            Self::TerminatedBySignal => "terminated_by_signal",
            Self::Failure => "failure",
        }
    }
}

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
    /// pattern — not machine-parseable. CLI drivers use the shared
    /// token-limit detector before projecting command errors to this variant.
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

    /// Fallback class for this error, or `None` for cancellation.
    ///
    /// Cancellation is iter's cooperative shutdown path and must always
    /// propagate instead of starting another agent attempt.
    #[must_use]
    pub fn fallback_class(&self) -> Option<FallbackClass> {
        match self {
            Self::Cancelled => None,
            Self::IterationTimeout(_) => Some(FallbackClass::Timeout),
            Self::TokenLimit(_) => Some(FallbackClass::TokenLimit),
            Self::Launch(_) => Some(FallbackClass::Launch),
            Self::TerminatedBySignal(_) => Some(FallbackClass::TerminatedBySignal),
            Self::Failed { .. } => Some(FallbackClass::Failure),
        }
    }

    /// Process exit code associated with this error, when one is meaningful.
    /// `None` for cancellation, timeout, token-limit, launch failure, and
    /// signal termination (a signal is not an exit code).
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Failed { code, .. } => *code,
            Self::Cancelled
            | Self::IterationTimeout(_)
            | Self::TokenLimit(_)
            | Self::Launch(_)
            | Self::TerminatedBySignal(_) => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_class_excludes_cancelled_and_classifies_failures() {
        assert_eq!(AgentError::Cancelled.fallback_class(), None);
        assert_eq!(
            AgentError::IterationTimeout(std::time::Duration::from_secs(1)).fallback_class(),
            Some(FallbackClass::Timeout),
        );
        assert_eq!(
            AgentError::TokenLimit("limit".to_owned()).fallback_class(),
            Some(FallbackClass::TokenLimit),
        );
        assert_eq!(
            AgentError::Launch("spawn".to_owned()).fallback_class(),
            Some(FallbackClass::Launch),
        );
        assert_eq!(
            AgentError::TerminatedBySignal(9).fallback_class(),
            Some(FallbackClass::TerminatedBySignal),
        );
        assert_eq!(
            AgentError::Failed {
                code: Some(1),
                message: "failed".to_owned(),
            }
            .fallback_class(),
            Some(FallbackClass::Failure),
        );
    }

    #[test]
    fn fallback_class_labels_match_error_labels() {
        let cases = [
            (
                AgentError::IterationTimeout(std::time::Duration::from_secs(1)),
                FallbackClass::Timeout,
            ),
            (
                AgentError::TokenLimit("limit".to_owned()),
                FallbackClass::TokenLimit,
            ),
            (
                AgentError::Launch("spawn".to_owned()),
                FallbackClass::Launch,
            ),
            (
                AgentError::TerminatedBySignal(9),
                FallbackClass::TerminatedBySignal,
            ),
            (
                AgentError::Failed {
                    code: Some(1),
                    message: "failed".to_owned(),
                },
                FallbackClass::Failure,
            ),
        ];

        for (error, class) in cases {
            assert_eq!(error.label(), class.label());
        }
    }
}
