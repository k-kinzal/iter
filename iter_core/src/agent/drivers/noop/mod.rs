//! [`NoopAgent`] — does nothing, exits immediately with success.

use crate::agent::error::AgentError;
use crate::agent::report::AgentReport;
use crate::{Agent, AgentRunContext};

/// Agent that does nothing.
///
/// Returns [`AgentReport::success()`] without touching the workspace,
/// writing to stdio, or sleeping. Useful for verifying workspace
/// setup/teardown, event handler wiring, runner overhead benchmarks,
/// and dry-running Iterfile configuration.
#[derive(Debug, Clone)]
pub struct NoopAgent;

impl Agent for NoopAgent {
    type Error = AgentError;

    async fn run(&self, _ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        Ok(AgentReport::success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExitStatus;
    use crate::agent::testutil::ctx;
    use crate::prompt::Prompt;
    use std::path::Path;

    #[tokio::test]
    async fn returns_success() {
        let agent = NoopAgent;
        let prompt = Prompt::from("ignored");
        let report = agent
            .run(ctx(Path::new("."), &prompt))
            .await
            .expect("run ok");
        assert_eq!(report.exit_status, ExitStatus::Success);
        assert!(report.last_output.is_none());
        assert!(report.turn_count.is_none());
    }

    #[tokio::test]
    async fn workspace_is_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let before: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read_dir")
            .collect();
        assert!(before.is_empty());

        let agent = NoopAgent;
        let prompt = Prompt::from("ignored");
        agent
            .run(ctx(tmp.path(), &prompt))
            .await
            .expect("run ok");

        let after: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read_dir")
            .collect();
        assert!(after.is_empty());
    }
}
