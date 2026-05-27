//! `AnyAgent` enum + the `build_agent` builder.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iter_core::agent::{
    AgentError, AgentMode as ImplAgentMode, ClaudeAgent, ClaudeSettings, ClineAgent, ClineSettings,
    CodexAgent, CodexSettings, CopilotAgent, CopilotSettings, CursorAgent, CursorSettings,
    GeminiAgent, GeminiSettings, GenericAgent, OpenCodeAgent, OpenCodeSettings,
};
use iter_core::workspace::sandbox::agent_requirements;
use iter_core::{Agent, AgentReport, AgentRunContext, SandboxRequirements};
use iter_language::{AgentDecl, AgentMode as AstAgentMode};
use thiserror::Error;

use crate::agent_router::{AgentRouter, RoutingStrategy};

/// Errors produced while building an [`AnyAgent`] from an [`AgentDecl`].
#[derive(Debug, Error)]
pub enum AgentBuildError {
    /// `agent generic { command = [] }` — a generic agent declaration with
    /// no command to invoke.
    #[error("agent generic requires a non-empty `command` array")]
    GenericEmptyCommand,

    /// `agent router { }` with no sub-agents.
    #[error("agent router requires at least one sub-agent")]
    RouterEmpty,

    /// A sub-agent inside a router failed to build.
    #[error("router sub-agent `{name}` failed to build: {source}")]
    RouterSubAgent {
        /// Name of the sub-agent that failed.
        name: String,
        /// Underlying build error.
        #[source]
        source: Box<AgentBuildError>,
    },
}

/// Enum dispatch wrapper over every concrete [`iter_core::Agent`]
/// implementation in the workspace.
#[derive(Debug, Clone)]
pub enum AnyAgent {
    /// Anthropic Claude Code agent.
    Claude(ClaudeAgent),
    /// `OpenAI` Codex agent.
    Codex(CodexAgent),
    /// Google Gemini agent.
    Gemini(GeminiAgent),
    /// GitHub Copilot agent.
    Copilot(CopilotAgent),
    /// Cursor agent.
    Cursor(CursorAgent),
    /// Cline agent.
    Cline(ClineAgent),
    /// `opencode` agent.
    OpenCode(OpenCodeAgent),
    /// Generic command-driven agent.
    Generic(GenericAgent),
    /// Multi-agent router with fallback or rotation strategy.
    Router(AgentRouter),
}

impl Agent for AnyAgent {
    type Error = AgentError;

    async fn run(&self, ctx: AgentRunContext<'_>) -> Result<AgentReport, Self::Error> {
        match self {
            Self::Claude(a) => a.run(ctx).await,
            Self::Codex(a) => a.run(ctx).await,
            Self::Gemini(a) => a.run(ctx).await,
            Self::Copilot(a) => a.run(ctx).await,
            Self::Cursor(a) => a.run(ctx).await,
            Self::Cline(a) => a.run(ctx).await,
            Self::OpenCode(a) => a.run(ctx).await,
            Self::Generic(a) => a.run(ctx).await,
            Self::Router(a) => a.run(ctx).await,
        }
    }
}

impl AnyAgent {
    /// Assemble the sandbox access profile for this agent.
    ///
    /// Dispatch happens here (at the compose layer, where the concrete
    /// agent variant is known) rather than on the [`Agent`] trait itself,
    /// because sandbox policy is a Workspace-side concern — see
    /// [`iter_core::workspace::sandbox::agent_requirements`].
    #[must_use]
    pub fn sandbox_requirements(&self) -> SandboxRequirements {
        match self {
            Self::Claude(a) => agent_requirements::claude(a),
            Self::Codex(_)
            | Self::Gemini(_)
            | Self::Copilot(_)
            | Self::Cursor(_)
            | Self::Cline(_)
            | Self::OpenCode(_)
            | Self::Generic(_) => SandboxRequirements::default(),
            Self::Router(r) => merge_sandbox_requirements(r.agents()),
        }
    }
}

fn merge_sandbox_requirements(agents: &[(String, AnyAgent)]) -> SandboxRequirements {
    let mut merged = SandboxRequirements::default();
    for (_name, agent) in agents {
        let reqs = agent.sandbox_requirements();
        merged.network_hosts.extend(reqs.network_hosts);
        merged.file_reads.extend(reqs.file_reads);
        merged.file_writes.extend(reqs.file_writes);
        merged.file_write_regexes.extend(reqs.file_write_regexes);
        merged.env_pass.extend(reqs.env_pass);
        merged.allow_signal = merged.allow_signal || reqs.allow_signal;
    }
    merged.network_hosts.sort();
    merged.network_hosts.dedup();
    merged.file_reads.sort();
    merged.file_reads.dedup();
    merged.file_writes.sort();
    merged.file_writes.dedup();
    merged.file_write_regexes.sort();
    merged.file_write_regexes.dedup();
    merged.env_pass.sort();
    merged.env_pass.dedup();
    merged
}

fn convert_mode(mode: AstAgentMode) -> ImplAgentMode {
    match mode {
        AstAgentMode::Interactive => ImplAgentMode::Interactive,
        AstAgentMode::Print => ImplAgentMode::Print,
    }
}

/// Build an [`AnyAgent`] from an [`AgentDecl`].
///
/// The compose layer is a pure 1:1 mapping: every field on the AST variant
/// flows through to the corresponding `*Settings` struct without any
/// defaults applied in between. Agent-operational knowledge (the canonical
/// Copilot subcommand, sandbox requirements, etc.) lives inside
/// `iter_core::agent::*` and is merged by those modules, not here.
///
/// # Errors
///
/// Returns [`AgentBuildError`] when the declaration is structurally invalid
/// for the chosen variant — currently only the `generic { command = [] }`
/// case.
pub fn build_agent(decl: &AgentDecl) -> Result<AnyAgent, AgentBuildError> {
    Ok(match decl {
        AgentDecl::Claude {
            mode,
            command,
            args,
            session_id_file,
            env,
        } => AnyAgent::Claude(ClaudeAgent::new(ClaudeSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            session_id_file: session_id_file.as_ref().map(PathBuf::from),
            env: resolve_env(env),
        })),
        AgentDecl::Codex {
            mode,
            command,
            args,
            env,
        } => AnyAgent::Codex(CodexAgent::new(CodexSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
        })),
        AgentDecl::Gemini {
            mode,
            command,
            args,
            env,
        } => AnyAgent::Gemini(GeminiAgent::new(GeminiSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
        })),
        AgentDecl::Copilot {
            mode,
            command,
            subcommand,
            args,
            env,
        } => AnyAgent::Copilot(CopilotAgent::new(CopilotSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            subcommand: subcommand.clone(),
            args: args.clone(),
            env: resolve_env(env),
        })),
        AgentDecl::Cursor { command, args, env } => {
            AnyAgent::Cursor(CursorAgent::new(CursorSettings {
                command: command.clone(),
                args: args.clone(),
                env: resolve_env(env),
            }))
        }
        AgentDecl::Cline { command, args, env } => {
            AnyAgent::Cline(ClineAgent::new(ClineSettings {
                command: command.clone(),
                args: args.clone(),
                env: resolve_env(env),
            }))
        }
        AgentDecl::OpenCode { command, args, env } => {
            AnyAgent::OpenCode(OpenCodeAgent::new(OpenCodeSettings {
                command: command.clone(),
                args: args.clone(),
                env: resolve_env(env),
            }))
        }
        AgentDecl::Generic { command, env } => {
            if command.is_empty() {
                return Err(AgentBuildError::GenericEmptyCommand);
            }
            let mut agent = GenericAgent::new(command.clone());
            agent.env = resolve_env(env);
            AnyAgent::Generic(agent)
        }
        AgentDecl::Router { agents, strategy } => build_router(agents, *strategy)?,
    })
}

fn build_router(
    agents: &[(String, Box<AgentDecl>)],
    strategy: iter_language::RouterStrategy,
) -> Result<AnyAgent, AgentBuildError> {
    if agents.is_empty() {
        return Err(AgentBuildError::RouterEmpty);
    }
    let routing_strategy = match strategy {
        iter_language::RouterStrategy::Fallback => RoutingStrategy::Fallback,
        iter_language::RouterStrategy::Rotate => RoutingStrategy::Rotate,
    };
    let mut built = Vec::with_capacity(agents.len());
    for (name, sub_decl) in agents {
        let sub_agent = build_agent(sub_decl).map_err(|e| AgentBuildError::RouterSubAgent {
            name: name.clone(),
            source: Box::new(e),
        })?;
        built.push((name.clone(), sub_agent));
    }
    Ok(AnyAgent::Router(AgentRouter::new(built, routing_strategy)))
}

/// Resolve declared env values with `ITER_` prefix overrides.
///
/// For every declared key `NAME`, if `ITER_NAME` is set in the runner
/// process environment, its value overrides the Iterfile default.
/// Undeclared `ITER_*` variables are ignored — only keys present in the
/// agent's `env` block participate.
fn resolve_env(declared: &BTreeMap<String, String>) -> Vec<(String, String)> {
    declared
        .iter()
        .map(|(key, default)| {
            let override_key = format!("ITER_{key}");
            let value = std::env::var(&override_key).unwrap_or_else(|_| default.clone());
            (key.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn empty_env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn claude_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Claude {
            mode,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
            env: empty_env(),
        }
    }

    fn codex_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Codex {
            mode,
            command: "codex".into(),
            args: Vec::new(),
            env: empty_env(),
        }
    }

    fn gemini_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Gemini {
            mode,
            command: "gemini".into(),
            args: Vec::new(),
            env: empty_env(),
        }
    }

    fn copilot_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Copilot {
            mode,
            command: "gh".into(),
            subcommand: None,
            args: Vec::new(),
            env: empty_env(),
        }
    }

    #[test]
    fn maps_each_agent_decl_variant() {
        type Check = fn(&AnyAgent) -> bool;
        let cases: [(AgentDecl, Check); 8] = [
            (claude_decl(AstAgentMode::Print), |a| {
                matches!(a, AnyAgent::Claude(_))
            }),
            (codex_decl(AstAgentMode::Print), |a| {
                matches!(a, AnyAgent::Codex(_))
            }),
            (gemini_decl(AstAgentMode::Print), |a| {
                matches!(a, AnyAgent::Gemini(_))
            }),
            (copilot_decl(AstAgentMode::Print), |a| {
                matches!(a, AnyAgent::Copilot(_))
            }),
            (
                AgentDecl::Cursor {
                    command: "cursor-agent".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                |a| matches!(a, AnyAgent::Cursor(_)),
            ),
            (
                AgentDecl::Cline {
                    command: "cline".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                |a| matches!(a, AnyAgent::Cline(_)),
            ),
            (
                AgentDecl::OpenCode {
                    command: "opencode".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                |a| matches!(a, AnyAgent::OpenCode(_)),
            ),
            (
                AgentDecl::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                },
                |a| matches!(a, AnyAgent::Generic(_)),
            ),
        ];
        for (decl, check) in &cases {
            let agent = build_agent(decl).expect("build");
            assert!(check(&agent), "wrong variant for {decl:?}");
        }
    }

    #[test]
    fn generic_with_empty_command_errors() {
        let err = build_agent(&AgentDecl::Generic {
            command: vec![],
            env: empty_env(),
        })
        .expect_err("must fail");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn agent_mode_passes_through_for_every_hook_capable_agent() {
        let claude = build_agent(&claude_decl(AstAgentMode::Interactive)).expect("build");
        match claude {
            AnyAgent::Claude(a) => assert_eq!(a.mode, ImplAgentMode::Interactive),
            other => panic!("expected Claude, got {other:?}"),
        }

        let codex = build_agent(&codex_decl(AstAgentMode::Interactive)).expect("build");
        match codex {
            AnyAgent::Codex(a) => assert_eq!(a.mode, ImplAgentMode::Interactive),
            other => panic!("expected Codex, got {other:?}"),
        }

        let gemini = build_agent(&gemini_decl(AstAgentMode::Interactive)).expect("build");
        match gemini {
            AnyAgent::Gemini(a) => assert_eq!(a.mode, ImplAgentMode::Interactive),
            other => panic!("expected Gemini, got {other:?}"),
        }

        let copilot = build_agent(&copilot_decl(AstAgentMode::Interactive)).expect("build");
        match copilot {
            AnyAgent::Copilot(a) => assert_eq!(a.mode, ImplAgentMode::Interactive),
            other => panic!("expected Copilot, got {other:?}"),
        }
    }

    #[test]
    fn command_and_args_pass_through_to_every_agent() {
        let claude = build_agent(&AgentDecl::Claude {
            mode: AstAgentMode::Print,
            command: "/opt/bin/claude".into(),
            args: vec!["--model".into(), "opus".into()],
            session_id_file: None,
            env: empty_env(),
        })
        .expect("build");
        match claude {
            AnyAgent::Claude(a) => {
                assert_eq!(a.command, "/opt/bin/claude");
                assert_eq!(a.args, vec!["--model".to_string(), "opus".into()]);
            }
            other => panic!("expected Claude, got {other:?}"),
        }

        let codex = build_agent(&AgentDecl::Codex {
            mode: AstAgentMode::Print,
            command: "/opt/bin/codex".into(),
            args: vec!["--model".into(), "o1".into()],
            env: empty_env(),
        })
        .expect("build");
        match codex {
            AnyAgent::Codex(a) => {
                assert_eq!(a.command, "/opt/bin/codex");
                assert_eq!(a.args, vec!["--model".to_string(), "o1".into()]);
            }
            other => panic!("expected Codex, got {other:?}"),
        }

        let gemini = build_agent(&AgentDecl::Gemini {
            mode: AstAgentMode::Print,
            command: "/opt/bin/gemini".into(),
            args: vec!["--sandbox".into()],
            env: empty_env(),
        })
        .expect("build");
        match gemini {
            AnyAgent::Gemini(a) => {
                assert_eq!(a.command, "/opt/bin/gemini");
                assert_eq!(a.args, vec!["--sandbox".to_string()]);
            }
            other => panic!("expected Gemini, got {other:?}"),
        }

        let copilot = build_agent(&AgentDecl::Copilot {
            mode: AstAgentMode::Print,
            command: "/opt/bin/copilot".into(),
            subcommand: Some(vec![]),
            args: vec!["--no-color".into()],
            env: empty_env(),
        })
        .expect("build");
        match copilot {
            AnyAgent::Copilot(a) => {
                assert_eq!(a.command, "/opt/bin/copilot");
                assert_eq!(a.subcommand.as_deref(), Some(&[] as &[String]));
                assert_eq!(a.args, vec!["--no-color".to_string()]);
            }
            other => panic!("expected Copilot, got {other:?}"),
        }

        let cursor = build_agent(&AgentDecl::Cursor {
            command: "/opt/bin/cursor".into(),
            args: vec!["--foo".into()],
            env: empty_env(),
        })
        .expect("build");
        match cursor {
            AnyAgent::Cursor(a) => {
                assert_eq!(a.command, "/opt/bin/cursor");
                assert_eq!(a.args, vec!["--foo".to_string()]);
            }
            other => panic!("expected Cursor, got {other:?}"),
        }

        let cline = build_agent(&AgentDecl::Cline {
            command: "/opt/bin/cline".into(),
            args: vec!["--bar".into()],
            env: empty_env(),
        })
        .expect("build");
        match cline {
            AnyAgent::Cline(a) => {
                assert_eq!(a.command, "/opt/bin/cline");
                assert_eq!(a.args, vec!["--bar".to_string()]);
            }
            other => panic!("expected Cline, got {other:?}"),
        }

        let opencode = build_agent(&AgentDecl::OpenCode {
            command: "/opt/bin/opencode".into(),
            args: vec!["--baz".into()],
            env: empty_env(),
        })
        .expect("build");
        match opencode {
            AnyAgent::OpenCode(a) => {
                assert_eq!(a.command, "/opt/bin/opencode");
                assert_eq!(a.args, vec!["--baz".to_string()]);
            }
            other => panic!("expected OpenCode, got {other:?}"),
        }
    }

    #[test]
    fn env_is_passed_through_to_agent() {
        let mut env = BTreeMap::new();
        env.insert("MY_VAR".to_string(), "my_value".to_string());
        let decl = AgentDecl::Claude {
            mode: AstAgentMode::Print,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
            env,
        };
        let agent = build_agent(&decl).expect("build");
        match agent {
            AnyAgent::Claude(a) => {
                assert_eq!(a.env, vec![("MY_VAR".to_string(), "my_value".to_string())]);
            }
            other => panic!("expected Claude, got {other:?}"),
        }
    }

    #[test]
    fn env_is_passed_through_to_generic_agent() {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let decl = AgentDecl::Generic {
            command: vec!["echo".into()],
            env,
        };
        let agent = build_agent(&decl).expect("build");
        match agent {
            AnyAgent::Generic(a) => {
                assert_eq!(a.env, vec![("FOO".to_string(), "bar".to_string())]);
            }
            other => panic!("expected Generic, got {other:?}"),
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn iter_prefix_overrides_declared_env() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut env = BTreeMap::new();
        env.insert("TEST_OVERRIDE".to_string(), "default".to_string());
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var("ITER_TEST_OVERRIDE", "overridden");
        }
        let resolved = resolve_env(&env);
        unsafe {
            std::env::remove_var("ITER_TEST_OVERRIDE");
        }
        assert_eq!(
            resolved,
            vec![("TEST_OVERRIDE".to_string(), "overridden".to_string())],
        );
    }

    #[test]
    #[allow(unsafe_code)]
    fn iter_prefix_uses_default_when_unset() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut env = BTreeMap::new();
        env.insert("UNIQUE_KEY_ZZZZ".to_string(), "default_val".to_string());
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var("ITER_UNIQUE_KEY_ZZZZ");
        }
        let resolved = resolve_env(&env);
        assert_eq!(
            resolved,
            vec![("UNIQUE_KEY_ZZZZ".to_string(), "default_val".to_string())],
        );
    }

    #[test]
    fn claude_session_id_file_is_forwarded() {
        let without = build_agent(&AgentDecl::Claude {
            mode: AstAgentMode::Print,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
            env: empty_env(),
        })
        .expect("build");
        match without {
            AnyAgent::Claude(a) => assert!(a.session_id_file.is_none()),
            other => panic!("expected Claude, got {other:?}"),
        }

        let with = build_agent(&AgentDecl::Claude {
            mode: AstAgentMode::Print,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: Some(".iter/session-id".into()),
            env: empty_env(),
        })
        .expect("build");
        match with {
            AnyAgent::Claude(a) => assert_eq!(
                a.session_id_file.as_deref(),
                Some(std::path::Path::new(".iter/session-id")),
            ),
            other => panic!("expected Claude, got {other:?}"),
        }
    }

    #[test]
    fn build_agent_router_fallback() {
        use iter_language::RouterStrategy;
        let decl = AgentDecl::Router {
            agents: vec![
                (
                    "primary".into(),
                    Box::new(AgentDecl::Claude {
                        mode: AstAgentMode::Print,
                        command: "claude".into(),
                        args: Vec::new(),
                        session_id_file: None,
                        env: empty_env(),
                    }),
                ),
                (
                    "secondary".into(),
                    Box::new(AgentDecl::Codex {
                        mode: AstAgentMode::Print,
                        command: "codex".into(),
                        args: Vec::new(),
                        env: empty_env(),
                    }),
                ),
            ],
            strategy: RouterStrategy::Fallback,
        };
        let agent = build_agent(&decl).expect("build");
        assert!(matches!(agent, AnyAgent::Router(_)));
    }

    #[test]
    fn build_agent_router_rotate() {
        use iter_language::RouterStrategy;
        let decl = AgentDecl::Router {
            agents: vec![(
                "only".into(),
                Box::new(AgentDecl::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                }),
            )],
            strategy: RouterStrategy::Rotate,
        };
        let agent = build_agent(&decl).expect("build");
        assert!(matches!(agent, AnyAgent::Router(_)));
    }

    #[test]
    fn build_agent_router_empty_errors() {
        use iter_language::RouterStrategy;
        let decl = AgentDecl::Router {
            agents: vec![],
            strategy: RouterStrategy::Fallback,
        };
        let err = build_agent(&decl).expect_err("must fail");
        assert!(err.to_string().contains("at least one"));
    }
}
