//! `AnyAgent` enum + the `build_agent` builder.

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

/// Errors produced while building an [`AnyAgent`] from an [`AgentDecl`].
#[derive(Debug, Error)]
pub enum AgentBuildError {
    /// `agent generic { command = [] }` — a generic agent declaration with
    /// no command to invoke.
    #[error("agent generic requires a non-empty `command` array")]
    GenericEmptyCommand,
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
        }
    }
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
        } => AnyAgent::Claude(ClaudeAgent::new(ClaudeSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            session_id_file: session_id_file.as_ref().map(PathBuf::from),
        })),
        AgentDecl::Codex {
            mode,
            command,
            args,
        } => AnyAgent::Codex(CodexAgent::new(CodexSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
        })),
        AgentDecl::Gemini {
            mode,
            command,
            args,
        } => AnyAgent::Gemini(GeminiAgent::new(GeminiSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
        })),
        AgentDecl::Copilot {
            mode,
            command,
            subcommand,
            args,
        } => AnyAgent::Copilot(CopilotAgent::new(CopilotSettings {
            command: command.clone(),
            mode: convert_mode(*mode),
            subcommand: subcommand.clone(),
            args: args.clone(),
        })),
        AgentDecl::Cursor { command, args } => AnyAgent::Cursor(CursorAgent::new(CursorSettings {
            command: command.clone(),
            args: args.clone(),
        })),
        AgentDecl::Cline { command, args } => AnyAgent::Cline(ClineAgent::new(ClineSettings {
            command: command.clone(),
            args: args.clone(),
        })),
        AgentDecl::OpenCode { command, args } => {
            AnyAgent::OpenCode(OpenCodeAgent::new(OpenCodeSettings {
                command: command.clone(),
                args: args.clone(),
            }))
        }
        AgentDecl::Generic { command } => {
            if command.is_empty() {
                return Err(AgentBuildError::GenericEmptyCommand);
            }
            AnyAgent::Generic(GenericAgent::new(command.clone()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Claude {
            mode,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
        }
    }

    fn codex_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Codex {
            mode,
            command: "codex".into(),
            args: Vec::new(),
        }
    }

    fn gemini_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Gemini {
            mode,
            command: "gemini".into(),
            args: Vec::new(),
        }
    }

    fn copilot_decl(mode: AstAgentMode) -> AgentDecl {
        AgentDecl::Copilot {
            mode,
            command: "gh".into(),
            subcommand: None,
            args: Vec::new(),
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
                },
                |a| matches!(a, AnyAgent::Cursor(_)),
            ),
            (
                AgentDecl::Cline {
                    command: "cline".into(),
                    args: Vec::new(),
                },
                |a| matches!(a, AnyAgent::Cline(_)),
            ),
            (
                AgentDecl::OpenCode {
                    command: "opencode".into(),
                    args: Vec::new(),
                },
                |a| matches!(a, AnyAgent::OpenCode(_)),
            ),
            (
                AgentDecl::Generic {
                    command: vec!["echo".into(), "hi".into()],
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
        let err = build_agent(&AgentDecl::Generic { command: vec![] }).expect_err("must fail");
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
    fn claude_session_id_file_is_forwarded() {
        let without = build_agent(&AgentDecl::Claude {
            mode: AstAgentMode::Print,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
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
}
