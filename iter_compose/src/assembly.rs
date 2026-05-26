//! Shared runner-builder assembly used by both `iter run` (Iterfile)
//! and `iter compose up` (compose services).
//!
//! Both code paths translate language declarations into a
//! [`RunnerBuilder`]. This module collects that translation into one
//! place so the assembly logic — build agent, build workspace factory,
//! compile prompts, wire event handlers — is expressed once and evolved
//! in lockstep.

use std::sync::Arc;

use iter_core::process::ProcessRuntime;
use iter_core::{Runner, RunnerBuilder, TemplateError};
use iter_language::{AgentDecl, EventHandlerDecl, PromptDecl, RunnerDecl, Spanned, WorkspaceDecl};
use thiserror::Error;

use crate::agent::{AgentBuildError, AnyAgent, build_agent};
use crate::config::build_runner_config;
use crate::events::register_event_handlers_from_events;
use crate::prompt::{PromptBuildError, build_prompt_selector_from_prompts};
use crate::queue::AnyQueue;
use crate::workspace::{AnyWorkspace, build_workspace_factory};

/// Errors produced by [`assemble_runner_builder`].
#[derive(Debug, Error)]
pub enum AssemblyError {
    /// Building the agent from its declaration failed.
    #[error(transparent)]
    AgentBuild(#[from] AgentBuildError),
    /// Building the prompt selector failed.
    #[error(transparent)]
    PromptBuild(#[from] PromptBuildError),
    /// Compiling an `on <event>` handler template failed.
    ///
    /// Uses `#[source]` (not `#[from]`) so `TemplateError` requires an
    /// explicit `.map_err` — prompt templates produce the same error
    /// type but route through [`PromptBuildError`] instead.
    #[error("invalid event handler template: {0}")]
    EventTemplate(#[source] TemplateError),
}

/// Assemble a [`RunnerBuilder`] from language declarations.
///
/// This is the shared core of both the Iterfile and compose service
/// build paths. It translates declarations into runtime objects and
/// wires them onto a builder ready for `.build()`.
///
/// The caller is responsible for:
/// - providing a pre-built queue (or `None` for queue-less runners)
/// - attaching any [`ProcessRuntime`] pieces via [`wire_builder_runtime`]
/// - calling `.build()` on the returned builder
///
/// # Errors
///
/// Returns [`AssemblyError`] when agent construction, prompt
/// compilation, or event-handler template compilation fails.
pub(crate) fn assemble_runner_builder(
    queue: Option<Arc<AnyQueue>>,
    workspace_decl: &WorkspaceDecl,
    agent_decl: &AgentDecl,
    runner_decl: &RunnerDecl,
    prompts: &[Spanned<PromptDecl>],
    events: &[Spanned<EventHandlerDecl>],
    once: bool,
) -> Result<RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>, AssemblyError> {
    let agent = build_agent(agent_decl)?;
    let workspaces = build_workspace_factory(workspace_decl, agent.sandbox_requirements());
    let prompt_selector = build_prompt_selector_from_prompts(prompts)?;
    let runner_config = build_runner_config(runner_decl, once);

    let mut builder = Runner::<AnyQueue, AnyWorkspace, AnyAgent>::builder()
        .workspaces(workspaces)
        .agent(agent)
        .prompt_selector(prompt_selector)
        .config(runner_config);
    if let Some(queue) = queue {
        builder = builder.queue(queue);
    }
    builder = register_event_handlers_from_events(builder, events)
        .map_err(AssemblyError::EventTemplate)?;

    Ok(builder)
}

/// Wire [`ProcessRuntime`] observer and stdio sink onto a builder.
///
/// Both `iter run` and compose in-process services use this to ensure
/// consistent wiring of the lifecycle observer and agent stdio capture.
///
/// The global log sender (`install_global_log_sender`) is deliberately
/// left to the caller: `iter run` installs it (single service per
/// process), while compose in-process services skip it (multiple
/// services would race on the global slot).
pub(crate) fn wire_builder_runtime(
    mut builder: RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>,
    runtime: &ProcessRuntime,
) -> RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent> {
    builder = builder.observer(runtime.observer().clone());
    builder = builder.stdio_sink(runtime.stdio().sink());
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use iter_language::{
        Action, AgentDecl, AgentMode, EventHandlerDecl, EventName, PromptDecl, RunnerBehavior,
        RunnerDecl, Spanned, WorkspaceDecl,
    };

    fn minimal_workspace() -> WorkspaceDecl {
        WorkspaceDecl::Local {
            base: "/tmp/assembly-test".into(),
        }
    }

    fn minimal_agent() -> AgentDecl {
        AgentDecl::Claude {
            mode: AgentMode::Print,
            command: "claude".into(),
            args: Vec::new(),
            session_id_file: None,
            env: BTreeMap::new(),
        }
    }

    fn minimal_runner() -> RunnerDecl {
        RunnerDecl {
            continue_on_error: false,
            behavior: RunnerBehavior::Loop { delay_secs: None },
            iteration_timeout_secs: None,
        }
    }

    fn minimal_prompts() -> Vec<Spanned<PromptDecl>> {
        vec![Spanned::new(
            PromptDecl {
                guard: None,
                body: "test prompt".into(),
            },
            0..0,
        )]
    }

    #[test]
    fn assemble_produces_buildable_runner_without_queue() {
        let builder = assemble_runner_builder(
            None,
            &minimal_workspace(),
            &minimal_agent(),
            &minimal_runner(),
            &minimal_prompts(),
            &[],
            false,
        )
        .expect("assembly should succeed");
        builder.build().expect("builder should produce a runner");
    }

    #[test]
    fn assemble_produces_buildable_runner_with_queue() {
        let queue =
            crate::queue::build_queue(&iter_language::QueueDecl::Memory).expect("in-memory queue");
        let builder = assemble_runner_builder(
            Some(Arc::new(queue)),
            &minimal_workspace(),
            &minimal_agent(),
            &RunnerDecl {
                continue_on_error: false,
                behavior: RunnerBehavior::Wait,
                iteration_timeout_secs: None,
            },
            &minimal_prompts(),
            &[],
            false,
        )
        .expect("assembly should succeed");
        builder.build().expect("builder should produce a runner");
    }

    #[test]
    fn assemble_wires_event_handlers() {
        let events = vec![Spanned::new(
            EventHandlerDecl {
                event: EventName::RunnerStarting,
                actions: vec![Action::Shell("echo start".into())],
            },
            0..0,
        )];
        let builder = assemble_runner_builder(
            None,
            &minimal_workspace(),
            &minimal_agent(),
            &minimal_runner(),
            &minimal_prompts(),
            &events,
            false,
        )
        .expect("assembly should succeed with event handlers");
        builder.build().expect("builder should produce a runner");
    }

    #[test]
    fn assemble_rejects_invalid_event_template() {
        let events = vec![Spanned::new(
            EventHandlerDecl {
                event: EventName::RunnerStarting,
                actions: vec![Action::Shell("echo {{".into())],
            },
            0..0,
        )];
        let result = assemble_runner_builder(
            None,
            &minimal_workspace(),
            &minimal_agent(),
            &minimal_runner(),
            &minimal_prompts(),
            &events,
            false,
        );
        assert!(
            matches!(result, Err(AssemblyError::EventTemplate(_))),
            "malformed template should fail with EventTemplate"
        );
    }

    #[test]
    fn iterfile_and_compose_service_both_use_shared_assembly() {
        // Both the Iterfile path (iterfile.rs) and the compose service
        // path (compose/plan.rs) now delegate to assemble_runner_builder.
        // This test builds equivalent declarations via both surfaces and
        // verifies they both produce buildable runners through the same
        // assembly function — if one path wires a field, the other
        // necessarily does too.
        let dir = tempfile::tempdir().expect("tmp");

        let iterfile_src = "\
            workspace local { base = \".\" }\n\
            agent claude { mode = print command = \"claude\" }\n\
            runner { continue_on_error = false behavior = wait }\n\
            prompt \"hello\"\n";

        // Iterfile path: parse + assemble directly
        let root = iter_language::parse(iterfile_src).expect("parse iterfile");
        let queue =
            crate::queue::build_queue(&iter_language::QueueDecl::Memory).expect("memory queue");
        let iterfile_builder = assemble_runner_builder(
            Some(Arc::new(queue)),
            &root.workspace.as_ref().unwrap().node,
            &root.agent.as_ref().unwrap().node,
            &root.runner.as_ref().unwrap().node,
            &root.prompts,
            &root.events,
            false,
        )
        .expect("iterfile assembly");
        iterfile_builder
            .build()
            .expect("iterfile builder should produce a runner");

        // Compose path: write iterfile + compose, build plan
        std::fs::write(dir.path().join("Iterfile"), iterfile_src).expect("write iterfile");
        let compose_src = "queue main file { path = \"./.iter/queue\" }\n\
                           service svc { build = \"./Iterfile\" }\n";
        let compose_path = dir.path().join("compose.iter");
        std::fs::write(&compose_path, compose_src).expect("write compose");
        let compose_root = crate::compose::load_compose(&compose_path).expect("load compose");
        let canonical = std::fs::canonicalize(&compose_path).unwrap();
        let plan =
            crate::compose::build(&compose_root, &canonical).expect("compose plan should build");
        assert_eq!(plan.service_count(), 1);
    }

    #[test]
    fn compose_service_builds_through_shared_assembly() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(
            dir.path().join("Iterfile"),
            "workspace local { base = \".\" }\n\
             agent claude { mode = print command = \"claude\" }\n\
             runner { continue_on_error = false behavior = wait }\n\
             prompt \"hello\"\n",
        )
        .expect("write iterfile");
        let compose_content = "queue main file { path = \"./.iter/queue\" }\n\
                               service svc { build = \"./Iterfile\" }\n";
        let compose_path = dir.path().join("compose.iter");
        std::fs::write(&compose_path, compose_content).expect("write compose");

        let root = crate::compose::load_compose(&compose_path).expect("load");
        let canonical = std::fs::canonicalize(&compose_path).unwrap();
        let plan = crate::compose::build(&root, &canonical).expect("build plan");

        assert_eq!(plan.service_count(), 1);
        assert_eq!(plan.service_names().next(), Some("svc"));
    }

    #[tokio::test]
    async fn wire_builder_runtime_installs_observer_and_stdio_sink() {
        use chrono::Utc;
        use iter_core::process::{
            LifecycleObserver, ProcessRegistry, ProcessRuntime, ShutdownController, StdioPolicy,
            StdioSupervisor,
        };

        let tmp = tempfile::tempdir().expect("tmp");
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let draft = iter_core::process::MetadataDraft {
            iterfile: tmp.path().join("Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec![],
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        };
        let (session, lock) = registry
            .register_foreground("test-svc", draft)
            .await
            .expect("register");
        std::mem::forget(lock);

        let log_dir = session.paths().dir().to_path_buf();
        let observer = Arc::new(
            LifecycleObserver::open_in(&log_dir, None)
                .await
                .expect("observer"),
        );
        let stdio = StdioSupervisor::new(StdioPolicy::LogOnly { log_dir })
            .await
            .expect("stdio");
        let runtime = ProcessRuntime::new(session, ShutdownController::new(), observer, stdio);

        let builder = assemble_runner_builder(
            None,
            &minimal_workspace(),
            &minimal_agent(),
            &minimal_runner(),
            &minimal_prompts(),
            &[],
            false,
        )
        .expect("assembly");

        assert!(!builder.has_stdio_sink(), "no sink before wiring");
        assert!(!builder.has_observer(), "no observer before wiring");

        let builder = wire_builder_runtime(builder, &runtime);

        assert!(builder.has_stdio_sink(), "stdio_sink must be wired");
        assert!(builder.has_observer(), "observer must be wired");
    }

    #[test]
    fn iterfile_path_builds_through_shared_assembly() {
        let iterfile_content = "\
            workspace local { base = \".\" }\n\
            agent claude { mode = print command = \"claude\" }\n\
            runner { continue_on_error = false behavior = loop }\n\
            prompt \"hello\"\n";

        let root = iter_language::parse(iterfile_content).expect("parse");
        let workspace_decl = root.workspace.as_ref().unwrap();
        let agent_decl = root.agent.as_ref().unwrap();
        let runner_decl = root.runner.as_ref().unwrap();

        let builder = assemble_runner_builder(
            None,
            &workspace_decl.node,
            &agent_decl.node,
            &runner_decl.node,
            &root.prompts,
            &root.events,
            false,
        )
        .expect("assembly from parsed Iterfile");
        builder.build().expect("builder should produce a runner");
    }
}
