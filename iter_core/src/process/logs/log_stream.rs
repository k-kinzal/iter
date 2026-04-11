//! NDJSON reader over the per-process `log.ndjson` file.
//!
//! Single responsibility: yield one [`LogEntry`] per non-empty line via
//! `serde_json::from_str`. Line tailing supports an optional `tail = N`
//! starting cap (skip-forward to the last `N` records) and `follow`
//! (poll for new lines after EOF) so it can back both `iter logs <id>`
//! and `iter logs <id> -f --tail N`.
//!
//! The on-disk file is written by
//! [`crate::process::stdio::LogJsonSink`]; both ends use [`LogEntry`] so
//! the schema stays in sync.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tokio::time::sleep;

use crate::process::error::{ProcessError, Result};
use crate::process::paths::names::LOG_NDJSON;

use super::{LogEntry, POLL_INTERVAL, io_err};

/// Tailing reader over a per-process `log.ndjson` file.
///
/// Construction reads any pre-existing entries up front so that
/// `tail = Some(N)` can return only the last `N` even after the file
/// has grown beyond `N` records. `follow == true` then keeps polling
/// for new lines after EOF; `follow == false` returns `Ok(None)` once
/// the file is fully drained.
pub struct LogStreamReader {
    reader: Option<BufReader<std::fs::File>>,
    path: PathBuf,
    follow: bool,
    /// Pre-loaded records the caller has not yet observed. Populated by
    /// `open` (with `tail = Some(N)` applied).
    buffered: VecDeque<LogEntry>,
    /// Last observed file size, used to detect truncation (file shrinks
    /// → seek to 0 and re-buffer).
    last_len: u64,
    /// Partial bytes already consumed from the file but not yet
    /// terminated by `\n`. The writer's `write_all` of `<json>\n` is not
    /// atomic at the syscall level — under `follow == true` the reader
    /// can race the writer between syscalls and observe the leading
    /// fragment of a record. Stashing it here lets the next poll
    /// concatenate the trailing bytes once the writer finishes the line,
    /// instead of feeding a truncated string to `serde_json::from_str`.
    partial: String,
}

impl std::fmt::Debug for LogStreamReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogStreamReader")
            .field("path", &self.path)
            .field("follow", &self.follow)
            .field("buffered", &self.buffered.len())
            .finish_non_exhaustive()
    }
}

impl LogStreamReader {
    /// Open a reader over `<dir>/log.ndjson`.
    ///
    /// `tail` caps the initial pre-load to the last `N` records; the
    /// reader still yields any subsequent records (in `follow` mode).
    ///
    /// # Errors
    ///
    /// Returns [`ProcessError::Io`] when the file exists but cannot be
    /// opened. A missing file is not an error: subsequent
    /// [`Self::next_entry`] calls return `Ok(None)` (or block until the
    /// file appears, in `follow` mode).
    pub fn open(dir: &Path, follow: bool, tail: Option<usize>) -> Result<Self> {
        let path = dir.join(LOG_NDJSON);
        let mut me = Self {
            reader: None,
            path,
            follow,
            buffered: VecDeque::new(),
            last_len: 0,
            partial: String::new(),
        };
        me.preload(tail)?;
        Ok(me)
    }

    fn ensure_open(&mut self) -> Result<bool> {
        if self.reader.is_some() {
            return Ok(true);
        }
        match std::fs::OpenOptions::new().read(true).open(&self.path) {
            Ok(f) => {
                self.reader = Some(BufReader::new(f));
                self.last_len = 0;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_err(e)),
        }
    }

    /// Drain every currently-available record into `buffered`. Detects
    /// truncation (file size shrank since last poll) and re-seeks to 0
    /// in that case.
    fn preload(&mut self, tail: Option<usize>) -> Result<()> {
        if !self.ensure_open()? {
            return Ok(());
        }
        while let Some(line) = self.read_one_line()? {
            if line.is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(&line).map_err(ProcessError::JsonRead)?;
            self.buffered.push_back(entry);
            if let Some(n) = tail {
                while self.buffered.len() > n {
                    self.buffered.pop_front();
                }
            }
        }
        Ok(())
    }

    /// Read one line from the underlying reader (without parsing).
    ///
    /// Returns `Ok(Some(line_without_newline))` for a full
    /// `\n`-terminated line, `Ok(None)` when EOF is reached *or* when the
    /// next line is incomplete (no terminating `\n` yet). In the
    /// incomplete case the bytes consumed so far are stashed in
    /// `self.partial`; the next call concatenates the continuation. This
    /// is the read-side defence against the writer's non-atomic
    /// `write_all` — see [`LogStreamReader::partial`].
    ///
    /// Detects file truncation and re-seeks to 0 (clearing any stash, as
    /// that data no longer exists).
    fn read_one_line(&mut self) -> Result<Option<String>> {
        if !self.ensure_open()? {
            return Ok(None);
        }
        let Some(reader) = self.reader.as_mut() else {
            return Ok(None);
        };
        // Truncation guard: if the file shrank since last observation,
        // start over from byte 0 and discard any stashed partial — the
        // bytes it referred to are gone.
        if let Ok(metadata) = reader.get_ref().metadata() {
            let len = metadata.len();
            if len < self.last_len {
                reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
                self.last_len = 0;
                self.partial.clear();
            }
        }
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(io_err)?;
        if n == 0 {
            // EOF: leave any stashed partial in place; the next poll
            // (in follow mode) will pick up the writer's continuation.
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
            // No terminator yet — the writer is mid-record. Stash and
            // signal EOF so the caller stops parsing; we'll resume on
            // the next read.
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
    /// Returns [`ProcessError::JsonRead`] when a line cannot be parsed
    /// as a [`LogEntry`], or [`ProcessError::Io`] for read failures.
    pub async fn next_entry(&mut self) -> Result<Option<LogEntry>> {
        loop {
            if let Some(entry) = self.buffered.pop_front() {
                return Ok(Some(entry));
            }
            // Drain any newly available lines without truncating to
            // tail — `tail` applied only to the initial preload.
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
    use crate::process::logs::LogStream;
    use chrono::Utc;
    use std::time::Duration;
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
            ts: Utc::now(),
            stream,
            line: line.into(),
        }
    }

    #[tokio::test]
    async fn reader_yields_entries_until_eof() {
        let tmp = TempDir::new().expect("tmp");
        let entries = vec![
            entry(LogStream::Stderr, "starting"),
            entry(LogStream::Stdout, "agent output"),
            entry(LogStream::Stderr, "agent finished"),
        ];
        write_entries(&tmp.path().join(LOG_NDJSON), &entries);

        let mut reader = LogStreamReader::open(tmp.path(), false, None).expect("open");
        for expected in &entries {
            let got = reader.next_entry().await.expect("ok").expect("some");
            assert_eq!(&got, expected);
        }
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_tail_keeps_only_last_n_entries() {
        let tmp = TempDir::new().expect("tmp");
        let entries: Vec<LogEntry> = (0..5)
            .map(|i| entry(LogStream::Stdout, &format!("line-{i}")))
            .collect();
        write_entries(&tmp.path().join(LOG_NDJSON), &entries);

        let mut reader = LogStreamReader::open(tmp.path(), false, Some(2)).expect("open");
        let a = reader.next_entry().await.expect("a").expect("some");
        let b = reader.next_entry().await.expect("b").expect("some");
        assert_eq!(a.line, "line-3");
        assert_eq!(b.line, "line-4");
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_returns_none_when_file_missing_and_no_follow() {
        let tmp = TempDir::new().expect("tmp");
        let mut reader = LogStreamReader::open(tmp.path(), false, None).expect("open");
        assert!(reader.next_entry().await.expect("eof").is_none());
    }

    #[tokio::test]
    async fn reader_skips_blank_lines() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join(LOG_NDJSON);
        let ev = entry(LogStream::Stderr, "hello");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\n");
        buf.extend_from_slice(&serde_json::to_vec(&ev).expect("ser"));
        buf.push(b'\n');
        std::fs::write(&path, buf).expect("write");

        let mut reader = LogStreamReader::open(tmp.path(), false, None).expect("open");
        let got = reader.next_entry().await.expect("ok").expect("some");
        assert_eq!(got, ev);
    }

    #[tokio::test]
    async fn reader_follow_picks_up_appended_entries() {
        use std::io::Write;
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join(LOG_NDJSON);
        let first = entry(LogStream::Stdout, "first");
        write_entries(&path, std::slice::from_ref(&first));

        let mut reader = LogStreamReader::open(tmp.path(), true, None).expect("open");
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
        let path = tmp.path().join(LOG_NDJSON);
        // Reproduce the writer's non-atomic write_all by appending the
        // first half of a record (no terminating \n) before the reader
        // opens, then the second half + \n later. Without the partial
        // stash the reader would feed the leading fragment to
        // serde_json::from_str and surface a JsonRead error.
        let ev = entry(LogStream::Stdout, "complete");
        let bytes = serde_json::to_vec(&ev).expect("ser");
        let split_at = bytes.len() / 2;
        let head = &bytes[..split_at];
        let tail = &bytes[split_at..];
        std::fs::write(&path, head).expect("write head");

        let mut reader = LogStreamReader::open(tmp.path(), true, None).expect("open");

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
