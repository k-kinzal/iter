//! Composition layer turning an [`iter_language::Iterfile`] into the concrete
//! types fed to [`iter_core::Runner`].
//!
//! This crate turns the open-ended world of "implementation crates" into a
//! concrete [`Runner`](iter_core::Runner). All three runtime axes are trait
//! objects — the queue (`Arc<dyn Queue>`), the workspace supply
//! (`Box<dyn Workspace>`), and the agent (`Box<dyn Agent>`); [`agent_from_def`]
//! selects and boxes the concrete driver for each agent definition.
//!
//! Trigger CLIs are separate binaries (`iter-cron`, `iter-watch`, etc.)
//! that connect to queues through the [`iter_core::queue`] boundary and
//! publish signals into them.

#![warn(missing_docs)]
#![deny(rust_2018_idioms)]

pub mod agent;
pub mod agent_router;
pub mod arg;
mod assembly;
pub mod compose;
pub mod config;
pub mod discovery;
pub mod events;
pub mod iterfile;
pub mod process_lifecycle;
pub mod project;
pub mod project_lock;
pub mod prompt;
pub mod queue;
pub mod secrets;
pub mod telemetry;
pub mod trigger_argv;
pub mod workspace;

pub use agent::{agent_from_def, sandbox_requirements_for};
pub use assembly::AssemblyError;
pub use compose::{
    CompletedTask, ComposeError, ComposePlan, ComposeReport, DEFAULT_COMPOSE_FILE, FailurePolicy,
    LABEL_ORCHESTRATOR_BOOT_ID, LABEL_ORCHESTRATOR_PID, LABEL_ORCHESTRATOR_START_TIME,
    LABEL_PROJECT, LABEL_SERVICE, OrchestratorContext, TargetedSpawnError, TriggerLifecycleState,
    TriggerRunError, TriggerStatus, build, is_compose_filename, load_compose, read_trigger_status,
    run, spawn_targeted_service, trigger_state_dir, trigger_state_root,
};
pub use config::build_runner_config;
pub use discovery::{
    ActiveOrchestrator, DiscoveryError, ProjectMember, find_active_orchestrator,
    list_all_members_by_project, list_project_members, open_default_registry,
};
pub use events::{register_event_handlers, register_event_handlers_from_events};
pub use process_lifecycle::{
    AdoptedBootstrapError, RunRecordMetadata, bootstrap_adopted, derive_finalize_reason,
    leaves_record_non_terminal, log_finalize_report,
};
pub use project::{ENV_PROJECT_NAME, ProjectSlugError, SlugValidationError, project_slug};
pub use project_lock::{ProjectLock, ProjectLockError, acquire_project_lock};
pub use prompt::{build_prompt_selector, build_prompt_selector_from_prompts};
pub use queue::{QueueBuildError, build_queue};
pub use secrets::resolve_secret;
pub use trigger_argv::queue_to_url;
pub use workspace::build_workspace_factory;
