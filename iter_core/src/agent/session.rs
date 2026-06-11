//! [`SessionIdFile`] — persistent agent session-id storage.
//!
//! Several agent CLIs (Claude Code's `--session-id`, Grok Build's
//! `-s/--session-id`) accept a caller-chosen session id that pins a
//! workspace to a stable conversation across iterations. This is the
//! narrowest exploration mode: the workspace, git history, and prior agent
//! context all bias the next iteration toward the same path. The user declares a
//! path in their Iterfile; iter reads the existing uuid from that path on
//! every run, or generates a fresh v4 uuid and writes it back on the first
//! run.
//!
//! The storage logic is agent-agnostic — only the flag the driver passes
//! the resolved id to (`--session-id`, `-s`, …) differs, and that lives in
//! each driver's command builder.

use std::path::{Path, PathBuf};

use crate::agent::AgentError;

/// A path that stores an agent session-id uuid across iter iterations.
/// Relative paths are resolved against the workspace the spawned agent
/// child will see, so a user-visible `".iter/session-id"` in an Iterfile
/// always points at the same file regardless of where iter is launched
/// from.
#[derive(Debug, Clone)]
pub(crate) struct SessionIdFile(PathBuf);

impl SessionIdFile {
    /// Wrap the declared path. The path is stored verbatim; resolution
    /// against the workspace happens inside [`Self::resolve`].
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Read the existing uuid from the file if present and non-empty,
    /// otherwise generate a fresh v4 uuid and write it (creating the
    /// parent directory if needed).
    pub(crate) async fn resolve(&self, workspace: &Path) -> Result<String, AgentError> {
        let absolute: PathBuf = if self.0.is_absolute() {
            self.0.clone()
        } else {
            workspace.join(&self.0)
        };

        if let Some(parent) = absolute.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }

        match tokio::fs::read_to_string(&absolute).await {
            Ok(existing) => {
                let trimmed = existing.trim();
                if !trimmed.is_empty() {
                    return Ok(trimmed.to_string());
                }
                // Empty file — fall through and regenerate.
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        let new_id = uuid::Uuid::new_v4().to_string();
        tokio::fs::write(&absolute, format!("{new_id}\n")).await?;
        Ok(new_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn resolve_reads_existing_file_verbatim() {
        let tmp = TempDir::new().expect("tmp");
        let fixed = "11111111-2222-4333-8444-555555555555";
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), format!("{fixed}\n"))
            .await
            .expect("seed");

        let file = SessionIdFile::new(PathBuf::from(".iter/session-id"));
        let got = file.resolve(tmp.path()).await.expect("resolve");
        assert_eq!(got, fixed);
    }

    #[tokio::test]
    async fn resolve_generates_uuid_and_writes_when_missing() {
        let tmp = TempDir::new().expect("tmp");
        let file = SessionIdFile::new(PathBuf::from(".iter/session-id"));
        let id = file.resolve(tmp.path()).await.expect("resolve");
        let parsed = uuid::Uuid::parse_str(&id).expect("uuid");
        assert_eq!(parsed.get_version_num(), 4);
        let persisted = fs::read_to_string(tmp.path().join(".iter/session-id"))
            .await
            .expect("read");
        assert_eq!(persisted.trim(), id);
    }

    #[tokio::test]
    async fn resolve_regenerates_when_file_is_empty() {
        let tmp = TempDir::new().expect("tmp");
        fs::create_dir_all(tmp.path().join(".iter"))
            .await
            .expect("mkdir");
        fs::write(tmp.path().join(".iter/session-id"), "")
            .await
            .expect("seed empty");
        let file = SessionIdFile::new(PathBuf::from(".iter/session-id"));
        let id = file.resolve(tmp.path()).await.expect("resolve");
        let parsed = uuid::Uuid::parse_str(&id).expect("uuid");
        assert_eq!(parsed.get_version_num(), 4);
    }
}
