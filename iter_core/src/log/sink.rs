//! Generic byte-sink trait for captured output streams.

use std::io;

use async_trait::async_trait;
use bytes::Bytes;

use super::LogStream;

/// Async sink for stdout/stderr chunks.
///
/// Implementations are expected to be cheap-to-clone (`Arc`) and free of
/// long-running blocking calls — local file appends and TTY writes only
/// per the design contract.
#[async_trait]
pub trait OutputSink: Send + Sync + 'static {
    /// Forward an owned chunk of stdout bytes.
    async fn write_stdout(&self, bytes: Bytes) -> io::Result<()>;
    /// Forward an owned chunk of stderr bytes.
    async fn write_stderr(&self, bytes: Bytes) -> io::Result<()>;
    /// Flush any buffered writes to the underlying medium.
    async fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    /// Flush the partial-line buffer for one stream only.
    ///
    /// Called from each agent-side tee task when its pipe reaches EOF.
    /// At that point the counterpart stream may still be writing, so we
    /// must not flush its in-flight partial along with ours.
    async fn flush_stream(&self, _stream: LogStream) -> io::Result<()> {
        Ok(())
    }
}

/// `OutputSink` that drops every byte.
#[derive(Debug, Default)]
pub struct NoopSink;

#[async_trait]
impl OutputSink for NoopSink {
    async fn write_stdout(&self, _bytes: Bytes) -> io::Result<()> {
        Ok(())
    }

    async fn write_stderr(&self, _bytes: Bytes) -> io::Result<()> {
        Ok(())
    }
}
