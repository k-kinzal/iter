//! Test-only helpers for exercising drivers through the real agent cycle.
//!
//! The crate's drivers all shell out to a real CLI binary; exercising them in
//! unit tests without `claude` / `codex` / `gemini` / etc. installed requires
//! writing a disposable shell script to a tempdir and pointing the driver's
//! `command` field at the script path. [`fake_binary_script`] wraps that
//! pattern, and [`drive`] / [`drive_capturing`] run a driver through the
//! full skeleton — a real [`LocalWorkspace`] active workspace, a
//! [`SingleAgentRouter`], and a fresh cancellation token.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::agent::Agent;
use crate::agent::driver::AgentDriver;
use crate::agent::router::SingleAgentRouter;
use crate::agent::{AgentError, AgentRun};
use crate::log::OutputSink;
use crate::prompt::Prompt;
use crate::workspace::{ActiveWorkspace, LocalWorkspace, Workspace};

/// Set up a [`LocalWorkspace`] over `path` and return its active form.
pub(crate) async fn active_local(path: &Path) -> Box<dyn ActiveWorkspace> {
    let mut ws = LocalWorkspace::new(path);
    // Call through the trait: the inherent `setup` returns the concrete
    // active type.
    Workspace::setup(&mut ws, CancellationToken::new())
        .await
        .expect("test workspace setup")
}

/// Run `driver` once through the full agent cycle on a local workspace at
/// `path`.
pub(crate) async fn drive(
    driver: impl AgentDriver + 'static,
    path: &Path,
    prompt: &Prompt,
) -> Result<AgentRun, AgentError> {
    let active = active_local(path).await;
    let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(driver))));
    agent
        .run_on(&*active, prompt, CancellationToken::new())
        .await
}

/// An [`OutputSink`] that records everything teed through it, so driver
/// tests can assert on the child's stdout/stderr. Mirrors what `log.ndjson`
/// would see.
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

/// Like [`drive`], but with a capturing stdio sink. Returns the run result
/// and the shared [`CaptureSink`] for assertions on the teed output.
pub(crate) async fn drive_capturing(
    driver: impl AgentDriver + 'static,
    path: &Path,
    prompt: &Prompt,
) -> (Result<AgentRun, AgentError>, Arc<CaptureSink>) {
    let sink = Arc::new(CaptureSink::default());
    let active = active_local(path).await;
    let agent = Agent::new(Box::new(SingleAgentRouter::new(Box::new(driver))))
        .with_stdio_sink(sink.clone());
    let result = agent
        .run_on(&*active, prompt, CancellationToken::new())
        .await;
    (result, sink)
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
