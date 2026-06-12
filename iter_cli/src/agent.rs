//! Definition → agent translation (`agent_from_def`).
//!
//! There is no `Any*` agent wrapper: the closed set of agent kinds lives in
//! the [`AgentDef`] definition enum, and the runtime drives a single
//! `Box<dyn Agent>` trait object (R18). [`agent_from_def`] is the one place
//! that selects a concrete driver from a definition and boxes it; dispatch at
//! run time is the vtable, not a match.
//!
//! The agent's sandbox profile is **not** built here: it is derived from the
//! constructed agent by
//! [`SandboxProfile::for_agent`](iter_core::SandboxProfile::for_agent), keyed
//! off the agent's own [`AgentKind`](iter_core::agent::AgentKind), so the
//! per-agent OS-access policy lives sandbox-side rather than at this boundary.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iter_core::Agent;
use iter_core::agent::{
    AgentMode as ImplAgentMode, AntigravityAgent, ClaudeAgent, ClineAgent, CodexAgent,
    CopilotAgent, CursorAgent, FakeAgent, GeminiAgent, GenericAgent, GrokAgent, HermesAgent,
    NoopAgent, OpenCodeAgent,
};
use iter_language::{AgentDef, AgentMode as AstAgentMode};
use thiserror::Error;

use iter_core::agent::{AgentRouter, RoutingStrategy};

/// Errors produced while translating an [`AgentDef`] into a boxed
/// [`Agent`](iter_core::Agent).
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

fn convert_mode(mode: AstAgentMode) -> ImplAgentMode {
    match mode {
        AstAgentMode::Interactive => ImplAgentMode::Interactive,
        AstAgentMode::Headless => ImplAgentMode::Headless,
    }
}

/// Build the concrete [`ClaudeAgent`] for a `AgentDef::Claude` definition.
///
/// Extracted so the field bind (declaration → driver) is expressed exactly
/// once, isolating the `String` → `PathBuf` session-path conversion that the
/// [`agent_from_def`] `Claude` arm boxes.
fn build_claude(
    mode: AstAgentMode,
    command: &str,
    args: &[String],
    session_id_file: Option<&String>,
    env: &BTreeMap<String, String>,
) -> ClaudeAgent {
    ClaudeAgent {
        command: command.to_owned(),
        mode: convert_mode(mode),
        args: args.to_vec(),
        session_id_file: session_id_file.map(PathBuf::from),
        env: resolve_env(env),
    }
}

/// Build the concrete [`GrokAgent`] for a `AgentDef::Grok` definition.
///
/// Extracted for the same reason as [`build_claude`].
fn build_grok(
    command: &str,
    args: &[String],
    session_id_file: Option<&String>,
    env: &BTreeMap<String, String>,
) -> GrokAgent {
    GrokAgent {
        command: command.to_owned(),
        args: args.to_vec(),
        session_id_file: session_id_file.map(PathBuf::from),
        env: resolve_env(env),
    }
}

/// Translate an [`AgentDef`] into the concrete driver it selects, boxed as a
/// `dyn Agent` trait object.
///
/// This is a pure selection-by-variant followed by a mechanical field move:
/// every field on the definition flows straight onto the corresponding agent
/// without defaults applied in between (agent-operational knowledge — the
/// canonical Copilot subcommand, the built-in CLI flags, sandbox requirements
/// — lives inside `iter_core::agent::*`, not here). The declaration `String`
/// session-id paths become core `PathBuf`s (a principled typing), and the
/// declared `env` map is resolved with `ITER_` overrides into the core
/// `Vec<(String, String)>`; no other reshaping happens at the boundary.
///
/// # Errors
///
/// Returns [`AgentBuildError`] when the definition is structurally invalid
/// for the chosen variant — the empty `generic { command = [] }` case and an
/// empty `router { }`.
#[allow(clippy::too_many_lines)]
pub fn agent_from_def(def: &AgentDef) -> Result<Box<dyn Agent>, AgentBuildError> {
    Ok(match def {
        AgentDef::Claude {
            mode,
            command,
            args,
            session_id_file,
            env,
        } => Box::new(build_claude(
            *mode,
            command,
            args,
            session_id_file.as_ref(),
            env,
        )),
        AgentDef::Codex {
            mode,
            command,
            args,
            env,
        } => Box::new(CodexAgent {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Gemini {
            mode,
            command,
            args,
            env,
        } => Box::new(GeminiAgent {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Hermes {
            mode,
            command,
            args,
            env,
        } => Box::new(HermesAgent {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Antigravity {
            mode,
            command,
            args,
            conversation_id,
            env,
        } => Box::new(AntigravityAgent {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            conversation_id: conversation_id.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Copilot {
            mode,
            command,
            subcommand,
            args,
            env,
        } => Box::new(CopilotAgent {
            command: command.clone(),
            mode: convert_mode(*mode),
            subcommand: subcommand.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Cursor { command, args, env } => Box::new(CursorAgent {
            command: command.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Cline { command, args, env } => Box::new(ClineAgent {
            command: command.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::OpenCode { command, args, env } => Box::new(OpenCodeAgent {
            command: command.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Grok {
            command,
            args,
            session_id_file,
            env,
        } => Box::new(build_grok(command, args, session_id_file.as_ref(), env)),
        AgentDef::Noop => Box::new(NoopAgent),
        AgentDef::Fake {
            exit_code,
            delay_secs,
            stdout,
            stderr,
            files,
        } => Box::new(FakeAgent {
            exit_code: *exit_code,
            delay_secs: delay_secs.unwrap_or(0),
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            files: files.clone(),
        }),
        AgentDef::Generic { command, env } => {
            if command.is_empty() {
                return Err(AgentBuildError::GenericEmptyCommand);
            }
            let mut agent = GenericAgent::new(command.clone());
            agent.env = resolve_env(env);
            Box::new(agent)
        }
        AgentDef::Router { agents, strategy } => Box::new(build_router(agents, *strategy)?),
    })
}

fn build_router(
    agents: &[(String, Box<AgentDef>)],
    strategy: iter_language::RouterStrategy,
) -> Result<AgentRouter, AgentBuildError> {
    if agents.is_empty() {
        return Err(AgentBuildError::RouterEmpty);
    }
    let routing_strategy = match strategy {
        iter_language::RouterStrategy::Fallback => RoutingStrategy::Fallback,
        iter_language::RouterStrategy::Rotate => RoutingStrategy::Rotate,
    };
    let mut built: Vec<(String, Box<dyn Agent>)> = Vec::with_capacity(agents.len());
    for (name, sub_def) in agents {
        let sub_agent = agent_from_def(sub_def).map_err(|e| AgentBuildError::RouterSubAgent {
            name: name.clone(),
            source: Box::new(e),
        })?;
        built.push((name.clone(), sub_agent));
    }
    Ok(AgentRouter::new(built, routing_strategy))
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

    fn claude_def(mode: AstAgentMode) -> AgentDef {
        AgentDef::Claude {
            mode,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
            env: empty_env(),
        }
    }

    /// The translation fn selects the right concrete driver for every
    /// definition variant. Identity is observed through the object-safe
    /// [`Agent::name`] accessor since the concrete type is erased behind the
    /// trait object — field-level bind coverage lives in each driver's own
    /// tests.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn agent_from_def_selects_each_variant() {
        let cases: [(AgentDef, &str); 13] = [
            (claude_def(AstAgentMode::Headless), "claude"),
            (
                AgentDef::Codex {
                    mode: AstAgentMode::Headless,
                    command: "codex".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "codex",
            ),
            (
                AgentDef::Gemini {
                    mode: AstAgentMode::Headless,
                    command: "gemini".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "gemini",
            ),
            (
                AgentDef::Hermes {
                    mode: AstAgentMode::Headless,
                    command: "hermes".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "hermes",
            ),
            (
                AgentDef::Antigravity {
                    mode: AstAgentMode::Headless,
                    command: "agy".into(),
                    args: Vec::new(),
                    conversation_id: None,
                    env: empty_env(),
                },
                "antigravity",
            ),
            (
                AgentDef::Copilot {
                    mode: AstAgentMode::Headless,
                    command: "gh".into(),
                    subcommand: None,
                    args: Vec::new(),
                    env: empty_env(),
                },
                "copilot",
            ),
            (
                AgentDef::Cursor {
                    command: "cursor-agent".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "cursor",
            ),
            (
                AgentDef::Cline {
                    command: "cline".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "cline",
            ),
            (
                AgentDef::OpenCode {
                    command: "opencode".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                "opencode",
            ),
            (
                AgentDef::Grok {
                    command: "grok".into(),
                    args: Vec::new(),
                    session_id_file: None,
                    env: empty_env(),
                },
                "grok",
            ),
            (
                AgentDef::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                },
                "generic",
            ),
            (AgentDef::Noop, "noop"),
            (
                AgentDef::Fake {
                    exit_code: 0,
                    delay_secs: None,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    files: BTreeMap::new(),
                },
                "fake",
            ),
        ];
        for (def, expected_name) in &cases {
            let agent = agent_from_def(def).expect("build");
            assert_eq!(agent.name(), *expected_name, "wrong driver for {def:?}");
        }
    }

    /// The Claude bind is a non-trivial field move: declaration `String`
    /// session paths become core `PathBuf`s, the AST mode maps to the core
    /// mode, and `args` pass through verbatim. `agent_from_def` erases the
    /// concrete type behind `Box<dyn Agent>`, so the bind is exercised through
    /// the shared `build_claude` constructor (which the `Claude` arm boxes).
    #[test]
    fn claude_def_binds_fields_including_session_path() {
        let mut env = BTreeMap::new();
        env.insert("BIND_TEST_KEY_ZZZ".to_string(), "v".to_string());
        let agent = build_claude(
            AstAgentMode::Interactive,
            "/opt/bin/claude",
            &["--model".to_string(), "opus".to_string()],
            Some(&".iter/session-id".to_string()),
            &env,
        );
        assert_eq!(agent.command, "/opt/bin/claude");
        assert_eq!(agent.mode, ImplAgentMode::Interactive);
        assert_eq!(agent.args, vec!["--model".to_string(), "opus".to_string()]);
        // Declaration `String` → core `PathBuf`.
        assert_eq!(
            agent.session_id_file,
            Some(PathBuf::from(".iter/session-id")),
        );
        // No `ITER_BIND_TEST_KEY_ZZZ` override is expected to exist, so the
        // declared default flows through the resolved env container.
        assert_eq!(
            agent.env,
            vec![("BIND_TEST_KEY_ZZZ".to_string(), "v".to_string())],
        );

        // Print mode and an absent session file bind to their counterparts.
        let none = build_claude(
            AstAgentMode::Headless,
            "claude",
            &[],
            None,
            &BTreeMap::new(),
        );
        assert_eq!(none.mode, ImplAgentMode::Headless);
        assert!(none.session_id_file.is_none());
    }

    /// Same non-trivial `String` → `PathBuf` session-path bind for Grok.
    #[test]
    fn grok_def_binds_session_path() {
        let with = build_grok(
            "grok",
            &["--output-format".to_string(), "json".to_string()],
            Some(&".iter/session-id".to_string()),
            &BTreeMap::new(),
        );
        assert_eq!(with.command, "grok");
        assert_eq!(
            with.args,
            vec!["--output-format".to_string(), "json".to_string()],
        );
        assert_eq!(
            with.session_id_file,
            Some(PathBuf::from(".iter/session-id")),
        );

        let without = build_grok("grok", &[], None, &BTreeMap::new());
        assert!(without.session_id_file.is_none());
    }

    #[test]
    fn generic_with_empty_command_errors() {
        // `Box<dyn Agent>` is not `Debug`, so `expect_err` (which would format
        // the `Ok` value) is unavailable; match the result explicitly instead.
        let Err(err) = agent_from_def(&AgentDef::Generic {
            command: vec![],
            env: empty_env(),
        }) else {
            panic!("empty generic command must fail to build");
        };
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn router_selects_router_driver() {
        use iter_language::RouterStrategy;
        let def = AgentDef::Router {
            agents: vec![
                (
                    "primary".into(),
                    Box::new(claude_def(AstAgentMode::Headless)),
                ),
                (
                    "secondary".into(),
                    Box::new(AgentDef::Codex {
                        mode: AstAgentMode::Headless,
                        command: "codex".into(),
                        args: Vec::new(),
                        env: empty_env(),
                    }),
                ),
            ],
            strategy: RouterStrategy::Fallback,
        };
        let agent = agent_from_def(&def).expect("build");
        assert_eq!(agent.name(), "router");
    }

    #[test]
    fn router_rotate_builds() {
        use iter_language::RouterStrategy;
        let def = AgentDef::Router {
            agents: vec![(
                "only".into(),
                Box::new(AgentDef::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                }),
            )],
            strategy: RouterStrategy::Rotate,
        };
        let agent = agent_from_def(&def).expect("build");
        assert_eq!(agent.name(), "router");
    }

    #[test]
    fn router_empty_errors() {
        use iter_language::RouterStrategy;
        let def = AgentDef::Router {
            agents: vec![],
            strategy: RouterStrategy::Fallback,
        };
        let Err(err) = agent_from_def(&def) else {
            panic!("empty router must fail to build");
        };
        assert!(err.to_string().contains("at least one"));
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
}
