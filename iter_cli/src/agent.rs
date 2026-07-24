//! Definition → agent translation (`agent_from_def`).
//!
//! There is no `Any*` agent wrapper: the closed set of agent kinds lives in
//! the [`AgentDef`] definition enum, and [`driver_from_def`] is the one
//! place that selects a concrete [`AgentDriver`] from a leaf definition and
//! boxes it. [`agent_from_def`] assembles the final [`Agent`]: every agent
//! holds exactly one router ([`SingleAgentRouter`] for a leaf definition,
//! [`FallbackRouter`] / [`RotateRouter`] for `kind = router`), so there is
//! no driver-or-router branch anywhere downstream.
//!
//! The sandbox profile is computed **here**, at the one moment the driver
//! list is still in hand — before it is composed into routers and the agent
//! (via [`SandboxProfile::for_drivers`]). The assembled agent is never
//! walked afterwards; `agent_from_def` returns the profile alongside the
//! agent so the start path can hand it to the workspace translation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iter_core::agent::{
    AgentDriver, AgentMode as ImplAgentMode, AntigravityDriver, ClaudeCodeDriver, ClineDriver,
    CodexDriver, CopilotDriver, CursorDriver, FakeDriver, FallbackClass, FallbackRouter,
    FallbackTriggers, GeminiDriver, GenericDriver, GrokDriver, HermesDriver, NoopDriver,
    OpenCodeDriver, RotateRouter, Router, SingleAgentRouter,
};
use iter_core::{Agent, SandboxProfile};
use iter_language::{
    AgentDef, AgentMode as AstAgentMode, RouterFallbackClass as AstFallbackClass,
    RouterFallbackTriggers as AstFallbackTriggers,
};
use thiserror::Error;

/// Hook isolation key for standalone starts. There is currently no operator
/// path that supplies a per-exploration value; compose is the intended
/// future producer.
const DEFAULT_HOOK_ISOLATION_KEY: &str = "default";

/// Errors produced while translating an [`AgentDef`] into an
/// [`Agent`](iter_core::Agent).
#[derive(Debug, Error)]
pub(crate) enum AgentBuildError {
    /// `agent generic { command = [] }` — a generic agent declaration with
    /// no command to invoke.
    #[error("agent generic requires a non-empty `command` array")]
    GenericEmptyCommand,

    /// `agent router { }` with no sub-agents.
    #[error("agent router requires at least one sub-agent")]
    RouterEmpty,

    /// A router nested inside a router. The language layer rejects this
    /// form; reaching it here means a semantic-layer regression.
    #[error("router sub-agents must be leaf agents")]
    RouterNested,

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

/// Build the concrete [`ClaudeCodeDriver`] for a `AgentDef::Claude`
/// definition.
///
/// Extracted so the field bind (declaration → driver) is expressed exactly
/// once, isolating the `String` → `PathBuf` session-path conversion that the
/// [`driver_from_def`] `Claude` arm boxes.
fn build_claude(
    mode: AstAgentMode,
    command: &str,
    args: &[String],
    system_prompt: Option<&String>,
    session_id_file: Option<&String>,
    output_schema: Option<&iter_language::OutputSchema>,
    env: &BTreeMap<String, String>,
) -> ClaudeCodeDriver {
    ClaudeCodeDriver {
        command: command.to_owned(),
        mode: convert_mode(mode),
        args: args.to_vec(),
        system_prompt: system_prompt.cloned(),
        session_id_file: session_id_file.map(PathBuf::from),
        output_schema: output_schema.map(|schema| schema.value.clone()),
        env: resolve_env(env),
        hook_isolation_key: DEFAULT_HOOK_ISOLATION_KEY.to_owned(),
    }
}

/// Build the concrete [`GrokDriver`] for a `AgentDef::Grok` definition.
///
/// Extracted for the same reason as [`build_claude`].
fn build_grok(
    command: &str,
    args: &[String],
    system_prompt: Option<&String>,
    session_id_file: Option<&String>,
    output_schema: Option<&iter_language::OutputSchema>,
    env: &BTreeMap<String, String>,
) -> GrokDriver {
    GrokDriver {
        command: command.to_owned(),
        args: args.to_vec(),
        system_prompt: system_prompt.cloned(),
        session_id_file: session_id_file.map(PathBuf::from),
        output_schema: output_schema.map(|schema| schema.value.clone()),
        env: resolve_env(env),
    }
}

/// Translate a **leaf** [`AgentDef`] into the concrete driver it selects,
/// boxed as a `dyn AgentDriver` trait object.
///
/// This is a pure selection-by-variant followed by a mechanical field move:
/// every field on the definition flows straight onto the corresponding driver
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
/// for the chosen variant — the empty `generic { command = [] }` case — or
/// when a `Router` definition reaches this leaf-only translation.
fn driver_from_def(def: &AgentDef) -> Result<Box<dyn AgentDriver>, AgentBuildError> {
    Ok(match def {
        AgentDef::Claude {
            mode,
            command,
            args,
            system_prompt,
            session_id_file,
            output_schema,
            env,
        } => Box::new(build_claude(
            *mode,
            command,
            args,
            system_prompt.as_ref(),
            session_id_file.as_ref(),
            output_schema.as_ref(),
            env,
        )),
        AgentDef::Codex {
            mode,
            command,
            args,
            output_schema,
            env,
        } => Box::new(CodexDriver {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            output_schema: output_schema.as_ref().map(|schema| schema.value.clone()),
            env: resolve_env(env),
            hook_isolation_key: DEFAULT_HOOK_ISOLATION_KEY.to_owned(),
        }),
        AgentDef::Gemini {
            mode,
            command,
            args,
            env,
        } => Box::new(GeminiDriver {
            command: command.clone(),
            mode: convert_mode(*mode),
            args: args.clone(),
            env: resolve_env(env),
            hook_isolation_key: DEFAULT_HOOK_ISOLATION_KEY.to_owned(),
        }),
        AgentDef::Hermes {
            mode,
            command,
            args,
            env,
        } => Box::new(HermesDriver {
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
        } => Box::new(AntigravityDriver {
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
        } => Box::new(CopilotDriver {
            command: command.clone(),
            mode: convert_mode(*mode),
            subcommand: subcommand.clone(),
            args: args.clone(),
            env: resolve_env(env),
            hook_isolation_key: DEFAULT_HOOK_ISOLATION_KEY.to_owned(),
        }),
        AgentDef::Cursor { command, args, env } => Box::new(CursorDriver {
            command: command.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Cline {
            command,
            args,
            system_prompt,
            env,
        } => Box::new(ClineDriver {
            command: command.clone(),
            args: args.clone(),
            system_prompt: system_prompt.clone(),
            env: resolve_env(env),
        }),
        AgentDef::OpenCode { command, args, env } => Box::new(OpenCodeDriver {
            command: command.clone(),
            args: args.clone(),
            env: resolve_env(env),
        }),
        AgentDef::Grok {
            command,
            args,
            system_prompt,
            session_id_file,
            output_schema,
            env,
        } => Box::new(build_grok(
            command,
            args,
            system_prompt.as_ref(),
            session_id_file.as_ref(),
            output_schema.as_ref(),
            env,
        )),
        AgentDef::Noop => Box::new(NoopDriver),
        AgentDef::Fake {
            exit_code,
            delay_secs,
            stdout,
            stderr,
            files,
        } => Box::new(FakeDriver {
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
            let mut driver = GenericDriver::new(command.clone());
            driver.env = resolve_env(env);
            Box::new(driver)
        }
        AgentDef::Router { .. } => return Err(AgentBuildError::RouterNested),
    })
}

/// Assemble the [`Agent`] (and its sandbox profile) for a definition.
///
/// A leaf definition becomes a [`SingleAgentRouter`] over its driver; a
/// `Router` definition becomes a [`FallbackRouter`] or [`RotateRouter`] over
/// its named drivers, in declaration order. The [`SandboxProfile`] is the
/// union over the same driver list, computed before the drivers move into
/// the router.
///
/// # Errors
///
/// Returns [`AgentBuildError`] for structurally invalid definitions (empty
/// generic command, empty router, a failing sub-agent).
pub(crate) fn agent_from_def(def: &AgentDef) -> Result<(Agent, SandboxProfile), AgentBuildError> {
    match def {
        AgentDef::Router {
            agents,
            strategy,
            fallback_on,
        } => {
            if agents.is_empty() {
                return Err(AgentBuildError::RouterEmpty);
            }
            let mut built: Vec<(String, Box<dyn AgentDriver>)> = Vec::with_capacity(agents.len());
            for (name, sub_def) in agents {
                let driver =
                    driver_from_def(sub_def).map_err(|e| AgentBuildError::RouterSubAgent {
                        name: name.clone(),
                        source: Box::new(e),
                    })?;
                built.push((name.clone(), driver));
            }
            let profile = SandboxProfile::for_drivers(built.iter().map(|(_, d)| d.as_ref()));
            let router: Box<dyn Router> = match strategy {
                iter_language::RouterStrategy::Fallback => Box::new(FallbackRouter::new(
                    built,
                    convert_fallback_triggers(fallback_on),
                )),
                iter_language::RouterStrategy::Rotate => Box::new(RotateRouter::new(built)),
            };
            Ok((Agent::new(router), profile))
        }
        leaf => {
            let driver = driver_from_def(leaf)?;
            let profile = SandboxProfile::for_drivers([driver.as_ref()]);
            Ok((
                Agent::new(Box::new(SingleAgentRouter::new(driver))),
                profile,
            ))
        }
    }
}

fn convert_fallback_triggers(triggers: &AstFallbackTriggers) -> FallbackTriggers {
    match triggers {
        AstFallbackTriggers::Any => FallbackTriggers::AnyFailure,
        AstFallbackTriggers::Only(classes) => FallbackTriggers::Only(
            classes
                .iter()
                .copied()
                .map(convert_fallback_class)
                .collect(),
        ),
    }
}

fn convert_fallback_class(class: AstFallbackClass) -> FallbackClass {
    match class {
        AstFallbackClass::Timeout => FallbackClass::Timeout,
        AstFallbackClass::TokenLimit => FallbackClass::TokenLimit,
        AstFallbackClass::Launch => FallbackClass::Launch,
        AstFallbackClass::TerminatedBySignal => FallbackClass::TerminatedBySignal,
        AstFallbackClass::Failure => FallbackClass::Failure,
    }
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
    use iter_core::agent::AgentKind;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn empty_env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn claude_def(mode: AstAgentMode) -> AgentDef {
        AgentDef::Claude {
            mode,
            command: "claude".into(),
            args: Vec::new(),
            system_prompt: None,
            session_id_file: None,
            output_schema: None,
            env: empty_env(),
        }
    }

    /// The translation fn selects the right concrete driver for every leaf
    /// definition variant. Identity is observed through the object-safe
    /// [`AgentDriver::kind`] fact since the concrete type is erased behind
    /// the trait object — field-level bind coverage lives in each driver's
    /// own tests.
    #[test]
    fn driver_from_def_selects_each_variant() {
        let cases: [(AgentDef, AgentKind); 13] = [
            (claude_def(AstAgentMode::Headless), AgentKind::Claude),
            (
                AgentDef::Codex {
                    mode: AstAgentMode::Headless,
                    command: "codex".into(),
                    args: Vec::new(),
                    output_schema: None,
                    env: empty_env(),
                },
                AgentKind::Codex,
            ),
            (
                AgentDef::Gemini {
                    mode: AstAgentMode::Headless,
                    command: "gemini".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                AgentKind::Gemini,
            ),
            (
                AgentDef::Hermes {
                    mode: AstAgentMode::Headless,
                    command: "hermes".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                AgentKind::Hermes,
            ),
            (
                AgentDef::Antigravity {
                    mode: AstAgentMode::Headless,
                    command: "agy".into(),
                    args: Vec::new(),
                    conversation_id: None,
                    env: empty_env(),
                },
                AgentKind::Antigravity,
            ),
            (
                AgentDef::Copilot {
                    mode: AstAgentMode::Headless,
                    command: "gh".into(),
                    subcommand: None,
                    args: Vec::new(),
                    env: empty_env(),
                },
                AgentKind::Copilot,
            ),
            (
                AgentDef::Cursor {
                    command: "cursor-agent".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                AgentKind::Cursor,
            ),
            (
                AgentDef::Cline {
                    command: "cline".into(),
                    args: Vec::new(),
                    system_prompt: None,
                    env: empty_env(),
                },
                AgentKind::Cline,
            ),
            (
                AgentDef::OpenCode {
                    command: "opencode".into(),
                    args: Vec::new(),
                    env: empty_env(),
                },
                AgentKind::OpenCode,
            ),
            (
                AgentDef::Grok {
                    command: "grok".into(),
                    args: Vec::new(),
                    system_prompt: None,
                    session_id_file: None,
                    output_schema: None,
                    env: empty_env(),
                },
                AgentKind::Grok,
            ),
            (
                AgentDef::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                },
                AgentKind::Generic,
            ),
            (AgentDef::Noop, AgentKind::Noop),
            (
                AgentDef::Fake {
                    exit_code: 0,
                    delay_secs: None,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    files: BTreeMap::new(),
                },
                AgentKind::Fake,
            ),
        ];
        for (def, expected_kind) in &cases {
            let driver = driver_from_def(def).expect("build");
            assert_eq!(driver.kind(), *expected_kind, "wrong driver for {def:?}");
        }
    }

    /// The Claude bind is a non-trivial field move: declaration `String`
    /// session paths become core `PathBuf`s, the AST mode maps to the core
    /// mode, and `args` pass through verbatim.
    #[test]
    fn claude_def_binds_fields_including_session_path() {
        let mut env = BTreeMap::new();
        env.insert("BIND_TEST_KEY_ZZZ".to_string(), "v".to_string());
        let driver = build_claude(
            AstAgentMode::Interactive,
            "/opt/bin/claude",
            &["--model".to_string(), "opus".to_string()],
            Some(&"Use Rust.".to_string()),
            Some(&".iter/session-id".to_string()),
            None,
            &env,
        );
        assert_eq!(driver.command, "/opt/bin/claude");
        assert_eq!(driver.mode, ImplAgentMode::Interactive);
        assert_eq!(driver.args, vec!["--model".to_string(), "opus".to_string()]);
        assert_eq!(driver.system_prompt.as_deref(), Some("Use Rust."));
        // Declaration `String` → core `PathBuf`.
        assert_eq!(
            driver.session_id_file,
            Some(PathBuf::from(".iter/session-id")),
        );
        // No `ITER_BIND_TEST_KEY_ZZZ` override is expected to exist, so the
        // declared default flows through the resolved env container.
        assert_eq!(
            driver.env,
            vec![("BIND_TEST_KEY_ZZZ".to_string(), "v".to_string())],
        );
        // Standalone starts pin the hook isolation key.
        assert_eq!(driver.hook_isolation_key, "default");

        // Print mode and an absent session file bind to their counterparts.
        let none = build_claude(
            AstAgentMode::Headless,
            "claude",
            &[],
            None,
            None,
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
            Some(&"Be concise.".to_string()),
            Some(&".iter/session-id".to_string()),
            None,
            &BTreeMap::new(),
        );
        assert_eq!(with.command, "grok");
        assert_eq!(
            with.args,
            vec!["--output-format".to_string(), "json".to_string()],
        );
        assert_eq!(with.system_prompt.as_deref(), Some("Be concise."));
        assert_eq!(
            with.session_id_file,
            Some(PathBuf::from(".iter/session-id")),
        );

        let without = build_grok("grok", &[], None, None, None, &BTreeMap::new());
        assert!(without.session_id_file.is_none());
    }

    #[test]
    fn generic_with_empty_command_errors() {
        let Err(err) = driver_from_def(&AgentDef::Generic {
            command: vec![],
            env: empty_env(),
        }) else {
            panic!("empty generic command must fail to build");
        };
        assert!(err.to_string().contains("non-empty"));
    }

    /// A router definition assembles an agent whose sandbox profile is the
    /// union over its drivers — the union computed here is the only place
    /// composition-wide OS access is decided.
    #[test]
    fn router_def_unions_driver_profiles() {
        use iter_language::{RouterFallbackTriggers, RouterStrategy};
        let def = AgentDef::Router {
            agents: vec![
                (
                    "primary".into(),
                    Box::new(claude_def(AstAgentMode::Headless)),
                ),
                (
                    "secondary".into(),
                    Box::new(AgentDef::Grok {
                        command: "grok".into(),
                        args: Vec::new(),
                        system_prompt: None,
                        session_id_file: None,
                        output_schema: None,
                        env: empty_env(),
                    }),
                ),
            ],
            strategy: RouterStrategy::Fallback,
            fallback_on: RouterFallbackTriggers::Any,
        };
        let (_agent, profile) = agent_from_def(&def).expect("build");
        assert!(profile.env_matches("ANTHROPIC_API_KEY"));
        assert!(profile.env_matches("XAI_API_KEY"));
    }

    #[test]
    fn router_rotate_builds() {
        use iter_language::{RouterFallbackTriggers, RouterStrategy};
        let def = AgentDef::Router {
            agents: vec![(
                "only".into(),
                Box::new(AgentDef::Generic {
                    command: vec!["echo".into(), "hi".into()],
                    env: empty_env(),
                }),
            )],
            strategy: RouterStrategy::Rotate,
            fallback_on: RouterFallbackTriggers::Any,
        };
        agent_from_def(&def).expect("build");
    }

    #[test]
    fn router_empty_errors() {
        use iter_language::{RouterFallbackTriggers, RouterStrategy};
        let def = AgentDef::Router {
            agents: vec![],
            strategy: RouterStrategy::Fallback,
            fallback_on: RouterFallbackTriggers::Any,
        };
        let Err(err) = agent_from_def(&def) else {
            panic!("empty router must fail to build");
        };
        assert!(err.to_string().contains("at least one"));
    }

    #[test]
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
        // SAFETY: serialised via ENV_LOCK; remove the temporary override
        // before leaving the test.
        unsafe {
            std::env::remove_var("ITER_TEST_OVERRIDE");
        }
        assert_eq!(
            resolved,
            vec![("TEST_OVERRIDE".to_string(), "overridden".to_string())],
        );
    }

    #[test]
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
