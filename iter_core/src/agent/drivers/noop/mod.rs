//! [`NoopAgent`] — does nothing, exits immediately with success.

use async_trait::async_trait;

use crate::agent::error::AgentError;
use crate::agent::run::AgentRun;
use crate::{Agent, AgentInvocation};

/// Agent that does nothing.
///
/// Returns an empty [`AgentRun`] without touching the workspace, writing to
/// stdio, or sleeping. Useful for verifying workspace setup/teardown, event
/// handler registration, runner overhead benchmarks, and dry-running a
/// declaration.
#[derive(Debug, Clone)]
pub struct NoopAgent;

#[async_trait]
impl Agent for NoopAgent {
    fn name(&self) -> &'static str {
        "noop"
    }

    async fn run(&self, _ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        Ok(AgentRun::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::ctx;
    use crate::prompt::Prompt;
    use std::path::Path;

    #[tokio::test]
    async fn returns_success() {
        let agent = NoopAgent;
        let prompt = Prompt::from("ignored");
        let run = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(run.session_id, None);
    }

    #[tokio::test]
    async fn workspace_is_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let before: Vec<_> = std::fs::read_dir(tmp.path()).expect("read_dir").collect();
        assert!(before.is_empty());

        let agent = NoopAgent;
        let prompt = Prompt::from("ignored");
        agent.run(ctx(tmp.path(), &prompt)).await.expect("run ok");

        let after: Vec<_> = std::fs::read_dir(tmp.path()).expect("read_dir").collect();
        assert!(after.is_empty());
    }
}
