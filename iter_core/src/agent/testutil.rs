//! Test-only helpers for building fake agent binaries at runtime.
//!
//! The crate's agents all shell out to a real CLI binary; exercising them in
//! unit tests without `claude` / `codex` / `gemini` / etc. installed requires
//! writing a disposable shell script to a tempdir and pointing the agent's
//! `command` field at the script path. [`fake_binary_script`] wraps that
//! pattern.

use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentRunContext;
use crate::prompt::Prompt;
use crate::signal::SignalId;

/// Build an [`AgentRunContext`] for unit tests with a fresh
/// [`CancellationToken`] and [`SignalId`].
pub(crate) fn ctx<'a>(path: &'a Path, prompt: &'a Prompt) -> AgentRunContext<'a> {
    AgentRunContext::new(path, prompt, CancellationToken::new(), SignalId::new())
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
