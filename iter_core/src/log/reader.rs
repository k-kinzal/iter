//! NDJSON reader over a `log.ndjson` file.
//!
//! Single responsibility: yield one [`LogEntry`] per non-empty line via
//! `serde_json::from_str`. Line tailing supports an optional `tail = N`
//! starting cap (skip-forward to the last `N` records) and `follow`
//! (poll for new lines after EOF).

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::sleep;

use super::LogEntry;

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Tailing reader over an NDJSON file of [`LogEntry`] records.
///
/// Construction reads any pre-existing entries up front so that
/// `tail = Some(N)` can return only the last `N` even after the file
/// has grown beyond `N` records. `follow == true` then keeps polling
/// for new lines after EOF; `follow == false` returns `Ok(None)` once
/// the file is fully drained.
pub struct NdjsonReader {
    reader: Option<BufReader<std::fs::File>>,
    path: PathBuf,
    follow: bool,
    buffered: VecDeque<LogEntry>,
    last_len: u64,
    partial: String,
}

impl std::fmt::Debug for NdjsonReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdjsonReader")
            .field("path", &self.path)
            .field("follow", &self.follow)
            .field("buffered", &self.buffered.len())
            .finish_non_exhaustive()
    }
}

/// Errors produced by [`NdjsonReader`].
#[derive(Debug, thiserror::Error)]
pub enum NdjsonReadError {
    /// An I/O error occurred while reading the file.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A line could not be parsed as a [`LogEntry`].
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

impl NdjsonReader {
    /// Open a reader over the given file path.
    ///
    /// `tail` caps the initial pre-load to the last `N` records; the
    /// reader still yields any subsequent records (in `follow` mode).
    ///
    /// A missing file is not an error: subsequent [`Self::next_entry`]
    /// calls return `Ok(None)` (or block until the file appears, in
    /// `follow` mode).
    ///
    /// # Errors
    ///
    /// Returns [`NdjsonReadError::Io`] when the file exists but cannot
    /// be opened, or [`NdjsonReadError::Json`] when a pre-loaded line
    /// cannot be parsed.
    pub fn open(path: &Path, follow: bool, tail: Option<usize>) -> Result<Self, NdjsonReadError> {
        let mut me = Self {
            reader: None,
            path: path.to_owned(),
            follow,
            buffered: VecDeque::new(),
            last_len: 0,
            partial: String::new(),
        };
        me.preload(tail)?;
        Ok(me)
    }

    fn ensure_open(&mut self) -> Result<bool, NdjsonReadError> {
        if self.reader.is_some() {
            return Ok(true);
        }
        match std::fs::OpenOptions::new().read(true).open(&self.path) {
            Ok(f) => {
                self.reader = Some(BufReader::new(f));
                self.last_len = 0;
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(NdjsonReadError::Io(e)),
        }
    }

    fn preload(&mut self, tail: Option<usize>) -> Result<(), NdjsonReadError> {
        if !self.ensure_open()? {
            return Ok(());
        }
        while let Some(line) = self.read_one_line()? {
            if line.is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(&line)?;
            self.buffered.push_back(entry);
            if let Some(n) = tail {
                while self.buffered.len() > n {
                    self.buffered.pop_front();
                }
            }
        }
        Ok(())
    }

    fn read_one_line(&mut self) -> Result<Option<String>, NdjsonReadError> {
        if !self.ensure_open()? {
            return Ok(None);
        }
        let Some(reader) = self.reader.as_mut() else {
            return Ok(None);
        };
        if let Ok(metadata) = reader.get_ref().metadata() {
            let len = metadata.len();
            if len < self.last_len {
                reader.seek(SeekFrom::Start(0))?;
                self.last_len = 0;
                self.partial.clear();
            }
        }
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        self.last_len += n as u64;
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
            if self.partial.is_empty() {
                Ok(Some(line))
            } else {
                let mut combined = std::mem::take(&mut self.partial);
                combined.push_str(&line);
                Ok(Some(combined))
            }
        } else {
            self.partial.push_str(&line);
            Ok(None)
        }
    }

    /// Yield the next complete record from the stream.
    ///
    /// Returns `Ok(None)` when EOF is reached and `follow == false`. In
    /// follow mode the future blocks indefinitely until a new record
    /// appears (or the future is dropped/cancelled).
    ///
    /// # Errors
    ///
    /// Returns [`NdjsonReadError::Json`] when a line cannot be parsed
    /// as a [`LogEntry`], or [`NdjsonReadError::Io`] for read failures.
    pub async fn next_entry(&mut self) -> Result<Option<LogEntry>, NdjsonReadError> {
        loop {
            if let Some(entry) = self.buffered.pop_front() {
                return Ok(Some(entry));
            }
            self.preload(None)?;
            if let Some(entry) = self.buffered.pop_front() {
                return Ok(Some(entry));
            }
            if self.follow {
                sleep(POLL_INTERVAL).await;
                continue;
            }
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::LogStream;
    use chrono::{DateTime, Utc};
    use std::time::Duration;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    fn write_entries(path: &Path, entries: &[LogEntry]) {
        let mut buf = Vec::new();
        for e in entries {
            buf.extend_from_slice(&serde_json::to_vec(e).expect("ser"));
            buf.push(b'\n');
        }
        std::fs::write(path, buf).expect("write");
    }

    fn entry(stream: LogStream, line: &str) -> LogEntry {
        LogEntry {
            ts: DateTime::<Utc>::from(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            stream,
            line: line.into(),
        }
    }

    #[tokio::test]
    async fn reader_yields_entries_until_eof() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let entries = vec![
            entry(LogStream::Stderr, "starting"),
            entry(LogStream::Stdout, "agent output"),
            entry(LogStream::Stderr, "agent finished"),
        ];
        write_entries(&path, &entries);

        let mut reader = NdjsonReader::open(&path, false, None).expect("open");
        for expected in &entries {
            let got = reader.next_entry().await.expect("ok").expect("some");
            assert_eq!(&got, expected);
        }
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_tail_keeps_only_last_n_entries() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let entries: Vec<LogEntry> = (0..5)
            .map(|i| entry(LogStream::Stdout, &format!("line-{i}")))
            .collect();
        write_entries(&path, &entries);

        let mut reader = NdjsonReader::open(&path, false, Some(2)).expect("open");
        let a = reader.next_entry().await.expect("a").expect("some");
        let b = reader.next_entry().await.expect("b").expect("some");
        assert_eq!(a.line, "line-3");
        assert_eq!(b.line, "line-4");
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_returns_none_when_file_missing_and_no_follow() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let mut reader = NdjsonReader::open(&path, false, None).expect("open");
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_skips_blank_lines() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let ev = entry(LogStream::Stderr, "hello");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\n");
        buf.extend_from_slice(&serde_json::to_vec(&ev).expect("ser"));
        buf.push(b'\n');
        std::fs::write(&path, buf).expect("write");

        let mut reader = NdjsonReader::open(&path, false, None).expect("open");
        let got = reader.next_entry().await.expect("ok").expect("some");
        assert_eq!(got, ev);
    }

    #[tokio::test]
    async fn reader_follow_picks_up_appended_entries() {
        use std::io::Write;
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let first = entry(LogStream::Stdout, "first");
        write_entries(&path, std::slice::from_ref(&first));

        let mut reader = NdjsonReader::open(&path, true, None).expect("open");
        let a = reader.next_entry().await.expect("a").expect("some");
        assert_eq!(a, first);

        let path_clone = path.clone();
        let appender = tokio::spawn(async move {
            sleep(Duration::from_millis(50)).await;
            let extra = entry(LogStream::Stderr, "second");
            let mut bytes = serde_json::to_vec(&extra).expect("ser");
            bytes.push(b'\n');
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path_clone)
                .expect("open append");
            f.write_all(&bytes).expect("append");
        });

        let got = tokio::time::timeout(Duration::from_secs(2), reader.next_entry())
            .await
            .expect("not timed out")
            .expect("ok");
        let entry = got.expect("some");
        assert_eq!(entry.line, "second");
        assert_eq!(entry.stream, LogStream::Stderr);
        appender.await.expect("join");
    }

    #[tokio::test]
    async fn reader_follow_waits_for_partial_record_to_complete() {
        use std::io::Write;
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("log.ndjson");
        let ev = entry(LogStream::Stdout, "complete");
        let bytes = serde_json::to_vec(&ev).expect("ser");
        let split_at = bytes.len() / 2;
        let head = &bytes[..split_at];
        let tail = &bytes[split_at..];
        std::fs::write(&path, head).expect("write head");

        let mut reader = NdjsonReader::open(&path, true, None).expect("open");

        let path_clone = path.clone();
        let tail_owned = tail.to_vec();
        let appender = tokio::spawn(async move {
            sleep(Duration::from_millis(50)).await;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path_clone)
                .expect("open append");
            f.write_all(&tail_owned).expect("append tail");
            f.write_all(b"\n").expect("append terminator");
        });

        let got = tokio::time::timeout(Duration::from_secs(2), reader.next_entry())
            .await
            .expect("not timed out")
            .expect("ok")
            .expect("some");
        assert_eq!(got, ev);
        appender.await.expect("join");
    }
}
