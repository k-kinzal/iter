//! [`FakeAgent`] — configurable fake agent for verification testing.
//!
//! Exercises real infrastructure (`OutputSink`, workspace filesystem,
//! cancellation) without requiring an external agent binary.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use bytes::Bytes;

use async_trait::async_trait;

use crate::agent::error::AgentError;
use crate::agent::run::AgentRun;
use crate::{Agent, AgentInvocation};

/// Configurable fake agent for verification testing.
///
/// Produces deterministic file changes, STDIO output, and exit status
/// through the real pipeline without requiring a real AI agent binary.
#[derive(Debug, Clone)]
pub struct FakeAgent {
    /// Process exit code. 0 = success, non-zero = failure.
    pub exit_code: i32,
    /// Simulated execution delay in seconds. 0 = immediate.
    pub delay_secs: u64,
    /// Lines to write to stdout via the [`OutputSink`](crate::log::OutputSink).
    pub stdout: Vec<String>,
    /// Lines to write to stderr via the [`OutputSink`](crate::log::OutputSink).
    pub stderr: Vec<String>,
    /// Files to create/overwrite in the workspace directory.
    pub files: BTreeMap<String, String>,
}

#[async_trait]
impl Agent for FakeAgent {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        for (path, content) in &self.files {
            let rel = Path::new(path);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|c| c == std::path::Component::ParentDir)
            {
                return Err(AgentError::Launch(format!(
                    "fake agent file path must be relative without `..`: {path}"
                )));
            }
            let full_path = ctx.workspace_path.join(rel);
            if let Some(parent) = full_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&full_path, content).await?;
        }

        for line in &self.stdout {
            let bytes = Bytes::from(format!("{line}\n"));
            ctx.stdio_sink.write_stdout(bytes).await?;
        }

        for line in &self.stderr {
            let bytes = Bytes::from(format!("{line}\n"));
            ctx.stdio_sink.write_stderr(bytes).await?;
        }

        if self.delay_secs > 0 {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(self.delay_secs)) => {}
                () = ctx.cancel.cancelled() => {
                    return Err(AgentError::Cancelled);
                }
            }
        }

        if self.exit_code == 0 {
            Ok(AgentRun::empty())
        } else {
            Err(AgentError::Failed {
                code: Some(self.exit_code),
                message: format!("fake agent exited with code {}", self.exit_code),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::ctx;
    use crate::prompt::Prompt;
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    fn default_fake_agent() -> FakeAgent {
        FakeAgent {
            exit_code: 0,
            delay_secs: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
            files: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn empty_config_behaves_like_noop() {
        let agent = default_fake_agent();
        let prompt = Prompt::from("ignored");
        let run = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(run.session_id, None);
    }

    #[tokio::test]
    async fn files_are_created_in_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut files = BTreeMap::new();
        files.insert("output/result.txt".to_string(), "content-a".to_string());
        files.insert("nested/deep/file.txt".to_string(), "content-b".to_string());
        let agent = FakeAgent {
            files,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("output/result.txt")).expect("read"),
            "content-a"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("nested/deep/file.txt")).expect("read"),
            "content-b"
        );
    }

    #[tokio::test]
    async fn exit_code_zero_is_success() {
        let agent = FakeAgent {
            exit_code: 0,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
    }

    #[tokio::test]
    async fn exit_code_nonzero_is_failure() {
        let agent = FakeAgent {
            exit_code: 1,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        let err = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect_err("nonzero exit is an error");
        assert!(
            matches!(err, AgentError::Failed { code: Some(1), .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn delay_respects_cancellation() {
        let cancel = CancellationToken::new();
        let agent = FakeAgent {
            delay_secs: 3600,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        let ctx = AgentInvocation::new(
            Path::new("."),
            &prompt,
            cancel.clone(),
            crate::signal::SignalId::new(),
        );
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });
        let err = agent.run(ctx).await.expect_err("must be cancelled");
        assert!(matches!(err, AgentError::Cancelled));
    }

    #[tokio::test]
    async fn absolute_path_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut files = BTreeMap::new();
        files.insert("/etc/passwd".to_string(), "bad".to_string());
        let agent = FakeAgent {
            files,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        let err = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect_err("must fail");
        assert!(matches!(err, AgentError::Launch(_)));
    }

    #[tokio::test]
    async fn parent_dir_traversal_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut files = BTreeMap::new();
        files.insert("../../escape.txt".to_string(), "bad".to_string());
        let agent = FakeAgent {
            files,
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        let err = agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect_err("must fail");
        assert!(matches!(err, AgentError::Launch(_)));
    }

    #[tokio::test]
    async fn stdout_lines_reach_stdio_sink() {
        use async_trait::async_trait;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        #[derive(Debug)]
        struct CaptureSink {
            stdout: Mutex<Vec<Bytes>>,
            stderr: Mutex<Vec<Bytes>>,
        }

        #[async_trait]
        impl crate::log::OutputSink for CaptureSink {
            async fn write_stdout(&self, bytes: Bytes) -> std::io::Result<()> {
                self.stdout.lock().await.push(bytes);
                Ok(())
            }
            async fn write_stderr(&self, bytes: Bytes) -> std::io::Result<()> {
                self.stderr.lock().await.push(bytes);
                Ok(())
            }
        }

        let sink = Arc::new(CaptureSink {
            stdout: Mutex::new(Vec::new()),
            stderr: Mutex::new(Vec::new()),
        });
        let agent = FakeAgent {
            stdout: vec!["hello".to_string(), "world".to_string()],
            stderr: vec!["warn".to_string()],
            ..default_fake_agent()
        };
        let prompt = Prompt::from("ignored");
        let run_ctx = AgentInvocation::new(
            Path::new("."),
            &prompt,
            CancellationToken::new(),
            crate::signal::SignalId::new(),
        )
        .with_stdio_sink(sink.clone());
        agent.run(run_ctx).await.expect("run ok");

        let stdout = sink.stdout.lock().await;
        assert_eq!(stdout.len(), 2);
        assert_eq!(&stdout[0][..], b"hello\n");
        assert_eq!(&stdout[1][..], b"world\n");

        let stderr = sink.stderr.lock().await;
        assert_eq!(stderr.len(), 1);
        assert_eq!(&stderr[0][..], b"warn\n");
    }

    #[tokio::test]
    async fn already_cancelled_returns_error() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let agent = default_fake_agent();
        let prompt = Prompt::from("ignored");
        let ctx = AgentInvocation::new(
            Path::new("."),
            &prompt,
            cancel,
            crate::signal::SignalId::new(),
        );
        let err = agent.run(ctx).await.expect_err("must be cancelled");
        assert!(matches!(err, AgentError::Cancelled));
    }
}
