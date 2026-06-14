//! Process-specific log wiring — policy, process-directory sink, and
//! the global sender for tracing fan-in.

pub(crate) mod policy;
pub(crate) mod sender;
pub(crate) mod sink;

pub(crate) use policy::OutputPolicy;
pub(crate) use sender::{LogSender, global_log_sender, install_global_log_sender};
pub(crate) use sink::{DEFAULT_LOG_BUFFER, ProcessLogSink};

use std::io;
use std::sync::Arc;

use iter_core::log::{NoopSink, OutputSink};

/// Bundled output sink and optional log sender produced by
/// [`open_output`].
///
/// The two pieces are always constructed together and travel together
/// through the bootstrap path into [`ProcessRuntime`](crate::process::ProcessRuntime).
pub(crate) struct ProcessOutput {
    sink: Arc<dyn OutputSink>,
    log_sender: Option<LogSender>,
}

impl ProcessOutput {
    /// Clone the [`Arc<dyn OutputSink>`] for distribution to agents.
    #[must_use]
    pub(crate) fn sink(&self) -> Arc<dyn OutputSink> {
        self.sink.clone()
    }

    /// Clone the [`LogSender`] when one exists. `None` for
    /// `Passthrough` policy.
    #[must_use]
    pub(crate) fn log_sender(&self) -> Option<LogSender> {
        self.log_sender.clone()
    }

    /// Consume the bundle into its parts. Used by
    /// [`ProcessRuntime::new`](crate::process::ProcessRuntime::new)
    /// which stores them as flat fields.
    pub(crate) fn into_parts(self) -> (Arc<dyn OutputSink>, Option<LogSender>) {
        (self.sink, self.log_sender)
    }

    /// Passthrough output for tests that don't need log capture.
    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self {
            sink: Arc::new(NoopSink),
            log_sender: None,
        }
    }
}

/// Construct an output sink and optional log sender from the given policy.
///
/// `LogOnly` opens [`ProcessLogSink`] and returns a sender wired to the
/// same NDJSON pipeline. `Passthrough` returns a [`NoopSink`] with no
/// sender.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] when the log file cannot be
/// opened (typically `ENOENT` or `EPERM`).
pub(crate) async fn open_output(policy: &OutputPolicy) -> io::Result<ProcessOutput> {
    match policy {
        OutputPolicy::Passthrough => Ok(ProcessOutput {
            sink: Arc::new(NoopSink),
            log_sender: None,
        }),
        OutputPolicy::LogOnly { log_dir } => {
            let sink = ProcessLogSink::open_in(log_dir).await?;
            let sender = sink.sender_handle();
            Ok(ProcessOutput {
                sink: Arc::new(sink),
                log_sender: Some(sender),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::TempDir;

    #[tokio::test]
    async fn open_output_passthrough_uses_noop_sink() {
        let output = open_output(&OutputPolicy::Passthrough).await.expect("open");
        assert!(output.log_sender.is_none());
        output.sink.write_stdout(Bytes::new()).await.expect("ok");
    }

    #[tokio::test]
    async fn open_output_log_only_opens_files_and_returns_sender() {
        use crate::process::paths::names::LOG_NDJSON;
        use iter_core::log::LogStream;

        let dir = TempDir::new().unwrap();
        let output = open_output(&OutputPolicy::LogOnly {
            log_dir: dir.path().to_owned(),
        })
        .await
        .expect("open");

        output
            .sink
            .write_stdout(Bytes::from_static(b"x\n"))
            .await
            .expect("ok");
        output.sink.flush().await.expect("flush");

        let body = std::fs::read_to_string(dir.path().join(LOG_NDJSON)).unwrap();
        let entries: Vec<iter_core::log::LogEntry> = body
            .lines()
            .map(|l| serde_json::from_str(l).expect("parse"))
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, "x");
        assert_eq!(entries[0].stream, LogStream::Stdout);
        assert!(output.log_sender.is_some());
    }
}
