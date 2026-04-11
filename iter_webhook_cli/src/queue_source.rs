//! `--queue-url` flag and the resolution that turns it into a runnable
//! queue handle.

use clap::Args;
use iter_trigger::{QueueHandle, QueueLoadError, QueueLoader};
use thiserror::Error;

use crate::error::{IntoExitCode, exit_codes};

#[derive(Debug, Error)]
pub(crate) enum QueueSourceError {
    #[error(transparent)]
    Load(#[from] QueueLoadError),
}

impl IntoExitCode for QueueSourceError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Load(_) => exit_codes::RUNTIME,
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
    pub(crate) async fn resolve(&self) -> Result<QueueHandle, QueueSourceError> {
        Ok(QueueLoader::from_url(&self.queue_url).await?)
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
        let q = probe.args.resolve().await.expect("memory");
        assert!(matches!(q, QueueHandle::Memory(_)));
    }
}
