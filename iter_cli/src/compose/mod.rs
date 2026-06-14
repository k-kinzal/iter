//! Top-level orchestration for `compose.iter`: load → build → run.
//!
//! This module is the compose-side counterpart to
//! [`crate::dispatch::run`]: it parses a `compose.iter` source file,
//! constructs every named queue and service declared in it,
//! and spawns them concurrently behind a single
//! [`tokio_util::sync::CancellationToken`].
//!
//! The split between [`build`] and [`run`] mirrors the Iterfile path
//! (`load_iterfile` → `Runner::build` → `Runner::run`): construction is
//! synchronous and fallible; the async loop runs only after every
//! pre-flight check succeeds.

mod error;
mod flatten;
mod plan;
mod run;
mod service;
mod service_build;
pub(crate) mod supervisor;
pub(crate) mod trigger;

use std::path::Path;

use iter_language::{Compose, parse_compose};

pub(crate) use error::{ComposeError, TargetedSpawnError};
pub(crate) use plan::{ComposePlan, build, build_single_service};
pub(crate) use run::{run, spawn_targeted_service};
pub(crate) use service::{
    CompletedServices, CompletedTask, FailurePolicy, LABEL_ORCHESTRATOR_BOOT_ID,
    LABEL_ORCHESTRATOR_PID, LABEL_ORCHESTRATOR_START_TIME, LABEL_PROJECT, LABEL_SERVICE,
    OrchestratorContext,
};
pub(crate) use supervisor::{
    TriggerLifecycleState, TriggerStatus, read_status as read_trigger_status, trigger_state_dir,
    trigger_state_root,
};
pub(crate) use trigger::TriggerRunError;

/// Default basename used by `iter compose` when no `-f` flag is supplied.
pub(crate) const DEFAULT_COMPOSE_FILE: &str = "compose.iter";

/// Return `true` when `path`'s basename identifies a compose file
/// (`compose.iter` or any `*.compose.iter`).
#[must_use]
pub(crate) fn is_compose_filename(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == DEFAULT_COMPOSE_FILE || n.ends_with(".compose.iter"))
}

/// Load and validate the `compose.iter` file at `path`.
///
/// # Errors
///
/// * The file does not exist or cannot be read.
/// * The parser produced one or more error-severity diagnostics.
pub(crate) fn load_compose(path: &Path) -> Result<Compose, ComposeError> {
    let source = std::fs::read_to_string(path).map_err(|e| ComposeError::io(path, e))?;
    parse_compose(&source).map_err(|diags| ComposeError::parse(path, &source, &diags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_compose_returns_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("compose.iter");
        std::fs::write(
            &path,
            r#"queue main file { path = "./.iter/queue" }

service worker {
    queue = main
    workspace_local { base = "." }
    agent_claude {
        mode = print
        command = "claude"
    }
    runner {
        continue_on_error = false
        behavior = wait
        prompt = "noop"
    }
}
"#,
        )
        .expect("write");
        let root = load_compose(&path).expect("load");
        assert_eq!(root.queues.len(), 1);
        assert_eq!(root.services.len(), 1);
    }

    #[test]
    fn inline_service_with_prompt_and_event_builds() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("compose.iter");
        std::fs::write(
            &path,
            r#"queue main file { path = "./.iter/queue" }

service worker {
    queue = main
    workspace_local { base = "." }
    agent_claude {
        mode = print
        command = "claude"
    }
    runner {
        continue_on_error = false
        behavior = loop
        prompt = "explore the workspace"
        on agent_finished { shell "echo done" }
    }
}
"#,
        )
        .expect("write");
        let root = load_compose(&path).expect("load");
        let canonical = std::fs::canonicalize(&path).expect("canonicalize");
        let plan = build(&root, &canonical).expect("inline service plan should build");
        assert_eq!(plan.service_count(), 1);
        assert_eq!(plan.service_names().next(), Some("worker"));
    }

    #[test]
    fn load_compose_renders_diagnostics_on_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("compose.iter");
        std::fs::write(&path, "queue main\n").expect("write");
        let err = load_compose(&path).expect_err("must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("queue") || msg.contains("backend"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn build_rejects_compose_with_zero_services() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("compose.iter");
        std::fs::write(
            &path,
            r#"queue main file { path = "./.iter/queue" }
"#,
        )
        .expect("write");
        let root = load_compose(&path).expect("load");
        let err = build(&root, &path).expect_err("zero services must reject");
        assert!(
            matches!(err, ComposeError::NoServices { .. }),
            "expected NoServices, got: {err:?}"
        );
    }
}
