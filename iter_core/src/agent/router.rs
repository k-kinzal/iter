//! `AgentRouter` — conditional agent dispatch per iteration.
//!
//! The router is itself an [`Agent`]: it composes named sub-agents
//! (`Vec<(String, Box<dyn Agent>)>`) and dispatches to one of them each
//! iteration according to a [`RoutingStrategy`]. The named-pair enumeration is
//! load-bearing — it backs routing and is kept un-flattened (never a bare
//! `Vec<Box<dyn Agent>>`) so each sub-agent stays individually addressable for
//! sandbox-profile introspection by name.
//!
//! For `RoutingStrategy::Fallback`, token-limit detection is implemented by
//! the CLI drivers before errors reach the router. The router fallback
//! predicate is configurable and defaults to every agent failure class except
//! cooperative cancellation.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::agent::{Agent, AgentError, AgentInvocation, AgentRun, FallbackClass};

/// Strategy governing how the router selects an agent each iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Use the first agent; on configured failure classes, try the next in order.
    Fallback,
    /// Rotate through agents round-robin across iterations.
    Rotate,
}

/// Failure classes that trigger fallback to the next router sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackTriggers {
    /// Fall back on every agent failure class except cancellation.
    AnyFailure,
    /// Fall back only on the listed failure classes.
    Only(HashSet<FallbackClass>),
}

impl Default for FallbackTriggers {
    fn default() -> Self {
        Self::AnyFailure
    }
}

impl FallbackTriggers {
    /// Return whether this trigger set includes the class.
    #[must_use]
    pub fn contains(&self, class: FallbackClass) -> bool {
        match self {
            Self::AnyFailure => true,
            Self::Only(classes) => classes.contains(&class),
        }
    }
}

/// A meta-agent that dispatches to one of several underlying agents based
/// on a [`RoutingStrategy`].
pub struct AgentRouter {
    agents: Vec<(String, Box<dyn Agent>)>,
    strategy: RoutingStrategy,
    fallback_triggers: FallbackTriggers,
    state: AtomicUsize,
}

impl AgentRouter {
    /// Construct a router over the given named agents with the specified strategy.
    ///
    /// # Panics
    ///
    /// Panics if `agents` is empty.
    #[must_use]
    pub fn new(
        agents: Vec<(String, Box<dyn Agent>)>,
        strategy: RoutingStrategy,
        fallback_triggers: FallbackTriggers,
    ) -> Self {
        assert!(
            !agents.is_empty(),
            "AgentRouter requires at least one agent"
        );
        Self {
            agents,
            strategy,
            fallback_triggers,
            state: AtomicUsize::new(0),
        }
    }

    /// Fallback advances on the configured failure classes. Cancellation
    /// always propagates and never advances to the next agent.
    async fn run_fallback(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        let mut last_err = None;
        for (i, (name, agent)) in self.agents.iter().enumerate() {
            let attempt_ctx = Self::subagent_ctx(&ctx, agent.as_ref());

            match agent.run(attempt_ctx).await {
                Ok(run) => return Ok(run),
                Err(err) => match err.fallback_class() {
                    Some(class) if self.fallback_triggers.contains(class) => {
                        let class_label = class.label();
                        tracing::warn!(
                            target: "iter::agent_router",
                            agent = name.as_str(),
                            index = i,
                            class = class_label,
                            "agent failed, trying next",
                        );
                        last_err = Some(err);
                    }
                    _ => return Err(err),
                },
            }
        }
        Err(last_err.expect("agents is non-empty"))
    }

    async fn run_rotate(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        let index = self.state.fetch_add(1, Ordering::Relaxed) % self.agents.len();
        let (_name, agent) = &self.agents[index];
        let attempt_ctx = Self::subagent_ctx(&ctx, agent.as_ref());
        agent.run(attempt_ctx).await
    }

    fn subagent_ctx<'a>(ctx: &AgentInvocation<'a>, agent: &'a dyn Agent) -> AgentInvocation<'a> {
        AgentInvocation::new(
            ctx.workspace_path,
            ctx.prompt,
            ctx.cancel.clone(),
            ctx.signal_id,
        )
        .with_signal_kind(ctx.signal_kind)
        .with_stdio_sink(ctx.stdio_sink.clone())
        .with_iteration_timeout(ctx.iteration_timeout)
        .with_hook_isolation_key(ctx.hook_isolation_key.clone())
        .with_sandbox_command_prefix(ctx.sandbox_command_prefix)
        .with_declared_env(agent.declared_env())
    }
}

#[async_trait]
impl Agent for AgentRouter {
    fn name(&self) -> &'static str {
        "router"
    }

    fn kind(&self) -> crate::agent::AgentKind {
        crate::agent::AgentKind::Router
    }

    /// The router's named sub-agents, in declaration order.
    ///
    /// Backs routing and — via the sandbox layer's `Router` match arm — the
    /// union of the sub-agents' sandbox profiles. The named-pair enumeration
    /// is deliberately kept un-flattened so each sub-agent stays individually
    /// addressable by name.
    fn sub_agents(&self) -> &[(String, Box<dyn Agent>)] {
        &self.agents
    }

    async fn run(&self, ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
        match self.strategy {
            RoutingStrategy::Fallback => self.run_fallback(ctx).await,
            RoutingStrategy::Rotate => self.run_rotate(ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Prompt;
    use crate::agent::{AgentKind, AgentMode, ClaudeAgent, GenericAgent};
    use crate::signal::SignalId;
    use std::collections::HashSet;
    use std::io::Write;
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    fn test_ctx(prompt: &Prompt) -> AgentInvocation<'_> {
        AgentInvocation::new(
            Path::new("."),
            prompt,
            CancellationToken::new(),
            SignalId::new(),
        )
    }

    fn generic(argv: Vec<String>) -> Box<dyn Agent> {
        Box::new(GenericAgent::new(argv))
    }

    struct CancelAgent;

    #[async_trait]
    impl Agent for CancelAgent {
        async fn run(&self, _ctx: AgentInvocation<'_>) -> Result<AgentRun, AgentError> {
            Err(AgentError::Cancelled)
        }

        fn kind(&self) -> AgentKind {
            AgentKind::Noop
        }
    }

    fn default_triggers() -> FallbackTriggers {
        FallbackTriggers::AnyFailure
    }

    fn token_limit_only_triggers() -> FallbackTriggers {
        FallbackTriggers::Only(HashSet::from([FallbackClass::TokenLimit]))
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
            mode: AgentMode::Headless,
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
        let router = AgentRouter::new(agents, RoutingStrategy::Rotate, default_triggers());
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
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
        let prompt = Prompt::from("x");
        router.run(test_ctx(&prompt)).await.expect("run ok");
    }

    #[tokio::test]
    async fn fallback_advances_past_launch_error_by_default() {
        let agents = vec![
            ("a".into(), generic(vec![])),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
        let prompt = Prompt::from("x");
        router
            .run(test_ctx(&prompt))
            .await
            .expect("fallback should succeed");
    }

    #[tokio::test]
    async fn fallback_propagates_cancelled_by_default() {
        let agents = vec![
            ("a".into(), Box::new(CancelAgent) as Box<dyn Agent>),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
        let prompt = Prompt::from("x");
        let err = router.run(test_ctx(&prompt)).await.unwrap_err();
        assert!(matches!(err, AgentError::Cancelled));
    }

    #[tokio::test]
    async fn fallback_advances_past_failed_error_by_default() {
        let agents = vec![
            ("a".into(), generic(vec!["false".into()])),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
        let prompt = Prompt::from("x");
        router
            .run(test_ctx(&prompt))
            .await
            .expect("fallback should succeed");
    }

    #[tokio::test]
    async fn fallback_token_limit_only_reproduces_legacy_behavior() {
        let agents = vec![
            ("a".into(), generic(vec![])),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(
            agents,
            RoutingStrategy::Fallback,
            token_limit_only_triggers(),
        );
        let prompt = Prompt::from("x");
        let err = router.run(test_ctx(&prompt)).await.unwrap_err();
        assert!(matches!(err, AgentError::Launch(_)));

        let tmp = tempfile::tempdir().unwrap();
        let script = token_limit_script(tmp.path());
        let agents = vec![
            ("a".into(), token_limit_agent(&script)),
            ("b".into(), generic(vec!["true".into()])),
        ];
        let router = AgentRouter::new(
            agents,
            RoutingStrategy::Fallback,
            token_limit_only_triggers(),
        );
        router
            .run(test_ctx(&prompt))
            .await
            .expect("token-limit fallback should still succeed");
    }

    #[tokio::test]
    async fn fallback_advances_past_token_limit_to_success() {
        let tmp = tempfile::tempdir().unwrap();
        let script = token_limit_script(tmp.path());
        let agent_a = token_limit_agent(&script);
        let agent_b = generic(vec!["true".into()]);

        let agents = vec![("a".into(), agent_a), ("b".into(), agent_b)];
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
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
        let router = AgentRouter::new(agents, RoutingStrategy::Fallback, default_triggers());
        let prompt = Prompt::from("x");
        let err = router.run(test_ctx(&prompt)).await.unwrap_err();
        assert!(
            matches!(err, AgentError::TokenLimit(ref detail) if detail.contains("context window")),
            "expected TokenLimit, got {err:?}",
        );
    }
}
