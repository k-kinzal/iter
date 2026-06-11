//! `AgentRouter` — conditional agent dispatch per iteration.
//!
//! The router is itself an [`Agent`]: it composes named sub-agents
//! (`Vec<(String, Box<dyn Agent>)>`) and dispatches to one of them each
//! iteration according to a [`RoutingStrategy`]. The named-pair enumeration is
//! load-bearing — it backs routing and is kept un-flattened (never a bare
//! `Vec<Box<dyn Agent>>`) so each sub-agent stays individually addressable for
//! sandbox-profile introspection by name.
//!
//! # Limitations
//!
//! `RoutingStrategy::Fallback` advances to the next agent only when the
//! current one returns `AgentError::TokenLimit`. Today only
//! `ClaudeAgent` in print mode performs token-limit detection; other
//! drivers surface the same condition as a non-zero exit code (which
//! the router treats as a normal completion). Detection for additional
//! drivers can be added incrementally in their respective modules.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use iter_core::agent::{AgentError, AgentRun};
use iter_core::{Agent, AgentRunContext};

/// Strategy governing how the router selects an agent each iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Use the first agent; on `TokenLimit` errors, try the next in order.
    Fallback,
    /// Rotate through agents round-robin across iterations.
    Rotate,
}

/// A meta-agent that dispatches to one of several underlying agents based
/// on a [`RoutingStrategy`].
pub struct AgentRouter {
    agents: Vec<(String, Box<dyn Agent>)>,
    strategy: RoutingStrategy,
    state: AtomicUsize,
}

impl AgentRouter {
    /// The named sub-agents this router dispatches to.
    ///
    /// Exposed as an object-safe `&[(String, Box<dyn Agent>)]`. The named-pair
    /// enumeration backs routing and is deliberately kept un-flattened so each
    /// sub-agent remains individually addressable for sandbox-profile
    /// introspection by name. (Compose currently keys the sandbox merge off
    /// the [`AgentDef`](iter_language::AgentDef) router definition, but the
    /// runtime enumeration is preserved for callers that introspect a built
    /// router.)
    #[must_use]
    pub fn agents(&self) -> &[(String, Box<dyn Agent>)] {
        &self.agents
    }

    /// Construct a router over the given named agents with the specified strategy.
    ///
    /// # Panics
    ///
    /// Panics if `agents` is empty.
    #[must_use]
    pub fn new(agents: Vec<(String, Box<dyn Agent>)>, strategy: RoutingStrategy) -> Self {
        assert!(
            !agents.is_empty(),
            "AgentRouter requires at least one agent"
        );
        Self {
            agents,
            strategy,
            state: AtomicUsize::new(0),
        }
    }

    /// Fallback triggers only on `AgentError::TokenLimit`. Any other failure
    /// (a non-zero exit surfaces as `AgentError::Failed`, a signal as
    /// `TerminatedBySignal`, etc.) is propagated as-is and does not advance
    /// to the next agent.
    async fn run_fallback(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let mut last_err = None;
        for (i, (name, agent)) in self.agents.iter().enumerate() {
            let attempt_ctx = AgentRunContext::new(
                ctx.workspace_path,
                ctx.prompt,
                ctx.cancel.clone(),
                ctx.signal_id,
            )
            .with_signal_kind(ctx.signal_kind)
            .with_stdio_sink(ctx.stdio_sink.clone())
            .with_iteration_timeout(ctx.iteration_timeout)
            .with_hook_isolation_key(ctx.hook_isolation_key.clone());

            match agent.run(attempt_ctx).await {
                Ok(run) => return Ok(run),
                Err(AgentError::TokenLimit(detail)) => {
                    tracing::warn!(
                        target: "iter::agent_router",
                        agent = name.as_str(),
                        index = i,
                        detail = detail.as_str(),
                        "agent hit token limit, trying next",
                    );
                    last_err = Some(AgentError::TokenLimit(detail));
                }
                Err(other) => return Err(other),
            }
        }
        Err(last_err.expect("agents is non-empty"))
    }

    async fn run_rotate(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        let index = self.state.fetch_add(1, Ordering::Relaxed) % self.agents.len();
        let (_name, agent) = &self.agents[index];
        agent.run(ctx).await
    }
}

#[async_trait]
impl Agent for AgentRouter {
    fn name(&self) -> &'static str {
        "router"
    }

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentRun, AgentError> {
        match self.strategy {
            RoutingStrategy::Fallback => self.run_fallback(ctx).await,
            RoutingStrategy::Rotate => self.run_rotate(ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::Prompt;
    use iter_core::agent::{AgentMode, ClaudeAgent, GenericAgent};
    use iter_core::signal::SignalId;
    use std::io::Write;
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    fn test_ctx(prompt: &Prompt) -> AgentRunContext<'_> {
        AgentRunContext::new(
            Path::new("."),
            prompt,
            CancellationToken::new(),
            SignalId::new(),
        )
    }

    fn generic(argv: Vec<String>) -> Box<dyn Agent> {
        Box::new(GenericAgent::new(argv))
    }

    fn token_limit_script(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("fake_agent.sh");
        let mut f = std::fs::File::create(&path).expect("create script");
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo 'context window exceeded' >&2").unwrap();
        writeln!(f, "exit 1").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn token_limit_agent(script: &Path) -> Box<dyn Agent> {
        Box::new(ClaudeAgent {
            command: script.to_str().unwrap().to_string(),
            mode: AgentMode::Print,
            args: Vec::new(),
            session_id_file: None,
            env: Vec::new(),
        })
    }

    #[tokio::test]
    async fn rotate_cycles_through_agents() {
        let agents = vec![
            ("a".into(), generic(vec!["true".into()])),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Rotate);
        let prompt = Prompt::from("x");

        for _ in 0..3 {
            router.run(test_ctx(&prompt)).await.expect("run ok");
        }
        assert_eq!(router.state.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn fallback_returns_first_success() {
        let agents = vec![
            ("a".into(), generic(vec!["true".into()])),
            ("b".into(), generic(vec!["false".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback);
        let prompt = Prompt::from("x");
        router.run(test_ctx(&prompt)).await.expect("run ok");
    }

    #[tokio::test]
    async fn fallback_propagates_non_token_limit_errors() {
        let agents = vec![
            ("a".into(), generic(vec![])),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback);
        let prompt = Prompt::from("x");
        let err = router.run(test_ctx(&prompt)).await.unwrap_err();
        assert!(matches!(err, AgentError::Launch(_)));
    }

    #[tokio::test]
    async fn fallback_advances_past_token_limit_to_success() {
        let tmp = tempfile::tempdir().unwrap();
        let script = token_limit_script(tmp.path());
        let agent_a = token_limit_agent(&script);
        let agent_b = generic(vec!["true".into()]);

        let agents = vec![("a".into(), agent_a), ("b".into(), agent_b)];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback);
        let prompt = Prompt::from("x");
        router
            .run(test_ctx(&prompt))
            .await
            .expect("fallback should succeed");
    }

    #[tokio::test]
    async fn fallback_exhaustion_returns_last_token_limit() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();
        let script_a = token_limit_script(tmp_a.path());
        let script_b = token_limit_script(tmp_b.path());

        let agents = vec![
            ("a".into(), token_limit_agent(&script_a)),
            ("b".into(), token_limit_agent(&script_b)),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback);
        let prompt = Prompt::from("x");
        let err = router.run(test_ctx(&prompt)).await.unwrap_err();
        assert!(
            matches!(err, AgentError::TokenLimit(ref detail) if detail.contains("context window")),
            "expected TokenLimit, got {err:?}",
        );
    }
}
