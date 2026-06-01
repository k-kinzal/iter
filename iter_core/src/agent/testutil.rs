//! Test-only helpers for building fake agent binaries at runtime.
//!
//! The crate's agents all shell out to a real CLI binary; exercising them in
//! unit tests without `claude` / `codex` / `gemini` / etc. installed requires
//! writing a disposable shell script to a tempdir and pointing the agent's
//! `command` field at the script path. [`fake_binary_script`] wraps that
//! pattern.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentRunContext;
use crate::log::OutputSink;
use crate::prompt::Prompt;
use crate::signal::SignalId;

/// Build an [`AgentRunContext`] for unit tests with a fresh
/// [`CancellationToken`] and [`SignalId`].
pub(crate) fn ctx<'a>(path: &'a Path, prompt: &'a Prompt) -> AgentRunContext<'a> {
    AgentRunContext::new(path, prompt, CancellationToken::new(), SignalId::new())
}

/// An [`OutputSink`] that records everything teed through it, so driver
/// tests can assert on the child's stdout/stderr now that the agent result
/// no longer carries an output tail. Mirrors what `log.ndjson` would see.
#[derive(Default)]
pub(crate) struct CaptureSink {
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
}

#[async_trait::async_trait]
impl OutputSink for CaptureSink {
    async fn write_stdout(&self, bytes: Bytes) -> std::io::Result<()> {
        self.stdout.lock().await.extend_from_slice(&bytes);
        Ok(())
    }
    async fn write_stderr(&self, bytes: Bytes) -> std::io::Result<()> {
        self.stderr.lock().await.extend_from_slice(&bytes);
        Ok(())
    }
}

impl CaptureSink {
    /// Captured stdout as a UTF-8 string.
    pub(crate) async fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().await).into_owned()
    }

    /// Captured stderr as a UTF-8 string.
    pub(crate) async fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().await).into_owned()
    }
}

/// Build an [`AgentRunContext`] whose stdio sink captures teed output.
/// Returns the context and the shared [`CaptureSink`] for later assertions.
pub(crate) fn ctx_capturing<'a>(
    path: &'a Path,
    prompt: &'a Prompt,
) -> (AgentRunContext<'a>, Arc<CaptureSink>) {
    let sink = Arc::new(CaptureSink::default());
    let ctx = AgentRunContext::new(path, prompt, CancellationToken::new(), SignalId::new())
        .with_stdio_sink(sink.clone());
    (ctx, sink)
}

/// Create an executable shell script in a fresh temp directory.
///
/// Returns the [`TempDir`] guard (keep it alive for the duration of the test)
/// and the absolute path to the script. The script's first line is a
/// `#!/bin/sh` shebang, followed by `body` verbatim.
pub(crate) fn fake_binary_script(body: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fake_agent.sh");
    {
        let mut f = std::fs::File::create(&path).expect("create script");
        writeln!(f, "#!/bin/sh").expect("write shebang");
        f.write_all(body.as_bytes()).expect("write body");
        writeln!(f).expect("trailing newline");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
    }
    (dir, path)
}
