//! `--queue-url` flag and the resolution that turns it into a runnable queue.
//!
//! The URL is parsed into a [`QueueDescriptor`] and connected through the core
//! [`connect`] boundary — the one place queue URLs are resolved.

use std::sync::Arc;

use clap::Args;
use iter_core::queue::{ConnectError, Queue, QueueAddressError, QueueDescriptor, connect};
use thiserror::Error;

use crate::error::{IntoExitCode, exit_codes};

#[derive(Debug, Error)]
pub(crate) enum QueueSourceError {
    /// The `--queue-url` could not be parsed as a queue address.
    #[error(transparent)]
    Address(#[from] QueueAddressError),
    /// Connecting to the queue named by `--queue-url` failed.
    #[error(transparent)]
    Connect(#[from] ConnectError),
}

impl IntoExitCode for QueueSourceError {
    fn exit_code(&self) -> i32 {
        // Both map to RUNTIME, matching the baseline `QueueLoader` behaviour:
        // this increment is a behaviour-preserving repoint, not an exit-code
        // change. (The `iter enqueue` surface keeps its own USER_INPUT mapping
        // for a malformed `--queue-url`, unchanged from its baseline.)
        match self {
            Self::Address(_) | Self::Connect(_) => exit_codes::RUNTIME,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct QueueSourceArgs {
    /// Queue connection URL (e.g. `memory://`, `file:///abs/path`,
    /// `redis://host:port`).
    #[arg(long = "queue-url", value_name = "URL")]
    pub(crate) queue_url: String,
}

impl QueueSourceArgs {
    pub(crate) async fn resolve(&self) -> Result<Arc<dyn Queue>, QueueSourceError> {
        let descriptor = QueueDescriptor::from_url(&self.queue_url)?;
        Ok(connect(&descriptor).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Probe {
        #[command(flatten)]
        args: QueueSourceArgs,
    }

    #[test]
    fn rejects_missing_queue_url() {
        let err = Probe::try_parse_from(["probe"]).expect_err("must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("--queue-url") || msg.contains("required"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn resolve_accepts_memory_url() {
        let probe = Probe::parse_from(["probe", "--queue-url", "memory://"]);
        let queue = probe.args.resolve().await.expect("memory");
        // A freshly-connected memory queue accepts an enqueue.
        queue
            .enqueue(
                iter_core::signal::Signal::new(iter_core::signal::Metadata::new()),
                iter_core::Priority::NORMAL,
            )
            .await
            .expect("enqueue");
    }

    #[tokio::test]
    async fn rejects_unsupported_scheme() {
        let probe = Probe::parse_from(["probe", "--queue-url", "ftp://nope"]);
        // `dyn Queue` is not `Debug`, so match rather than `expect_err`.
        match probe.args.resolve().await {
            Err(QueueSourceError::Address(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("expected an unsupported-scheme error"),
        }
    }
}
