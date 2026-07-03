use std::io;

use thiserror::Error;

/// Errors produced while rendering or applying sandbox command wrappers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// No sandbox command is available for the compilation target.
    #[error("no sandbox command for this platform")]
    UnsupportedPlatform,

    /// The policy cannot be faithfully represented by the selected sandbox
    /// command.
    #[error("sandbox policy unsupported by {command}: {reason}")]
    UnsupportedPolicy {
        /// Sandbox command name, such as `bwrap` or `sandbox-exec`.
        command: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    /// Command wrapping failed because the command was malformed.
    #[error("invalid command: {0}")]
    InvalidCommand(String),

    /// I/O error.
    #[error("sandbox I/O error: {0}")]
    Io(#[from] io::Error),
}
