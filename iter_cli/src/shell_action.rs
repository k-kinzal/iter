//! [`ShellAction`] — execute `shell "..."` actions for `on <event> {}`
//! blocks.
//!
//! The action runs a shell command when the dispatcher routes an event it was
//! registered for. Because the [`EventDispatcher`](iter_core::EventDispatcher)
//! routes by [`EventName`](iter_core::EventName), the action itself carries no
//! event-name field — it is a pure action callback.
//!
//! The concrete `on { shell … }` action is an operator-configured side effect
//! (`sh -c`), not one of the six core concepts; it lives in the operator
//! surface (cli) and renders against core's public
//! [`Template`](iter_core::Template) and render views.
//!
//! # Template rendering
//!
//! The command string is compiled once into a [`Template`] and rendered
//! per-event against an [`IterationRenderContext`] — the same machinery the
//! runner uses for prompts. Template variables include `{{signal.id}}`,
//! `{{signal.created_at}}`, `{{today}}`, every `{{metadata.*}}` key attached
//! to the signal, and the per-iteration `{{iteration.*}}` snapshot.
//! Signal-less lifecycle events (`runner_starting`, `runner_finished`,
//! `runner_error` raised before a signal was dequeued) render against a
//! [`RunnerRenderContext`] so `{{signal.*}}` and `{{metadata.*}}` are
//! deliberately absent.
//!
//! # Working directory
//!
//! When the triggering event carries a workspace path (everything after
//! `workspace_setup_finished`), the shell command runs with that path as its
//! cwd. Events without a workspace path (`runner_starting`, `runner_finished`,
//! `signal_received`, `workspace_setup_starting`, `runner_error`) inherit the
//! parent's cwd.
//!
//! Shell commands run via `sh -c <cmd>` and inherit the parent's stdio. A
//! non-zero exit status is *logged* but never propagated back to the runner —
//! the [`EventDispatcher`](iter_core::EventDispatcher) contract calls event
//! actions on a best-effort basis.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use iter_core::{
    BoxError, EventAction, HookEvent, IterationContext, IterationRenderContext,
    RunnerRenderContext, Signal, Template, TemplateError,
};
use tokio::process::Command;
use tracing::warn;

/// Event action that runs a shell command.
///
/// The action holds only the work — the compiled command template and
/// execution logic. Which event it responds to is the dispatcher's
/// responsibility at registration time.
#[derive(Debug, Clone)]
pub struct ShellAction {
    command_source: String,
    compiled: Template,
}

impl ShellAction {
    /// Build an action that runs `command` when invoked.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] if `command` is not a valid
    /// Handlebars template.
    pub fn new(command: impl Into<String>) -> Result<Self, TemplateError> {
        let command_source = command.into();
        let compiled = Template::compile(command_source.clone())?;
        Ok(Self {
            command_source,
            compiled,
        })
    }

    async fn run_shell(&self, rendered: &str, cwd: Option<&Path>) -> Result<(), BoxError> {
        let mut command = Command::new("sh");
        command.arg("-c").arg(rendered).stdin(Stdio::null());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let status = command
            .status()
            .await
            .map_err(|e| -> BoxError { Box::new(e) })?;
        if !status.success() {
            warn!(
                command = %rendered,
                cwd = ?cwd,
                exit = ?status.code(),
                "shell action exited non-zero"
            );
        }
        Ok(())
    }
}

impl EventAction for ShellAction {
    async fn handle(
        &self,
        event: &HookEvent,
        iteration: &IterationContext,
    ) -> Result<(), BoxError> {
        let (signal, cwd) = extract_context(event);
        let render_result = match signal {
            Some(signal) => {
                let ctx = IterationRenderContext::new(signal, iteration);
                self.compiled.render(&ctx)
            }
            None => self.compiled.render(&RunnerRenderContext::new(iteration)),
        };
        let rendered = match render_result {
            Ok(text) => text,
            Err(err) => {
                warn!(
                    command = %self.command_source,
                    error = %err,
                    "shell action template render failed"
                );
                return Ok(());
            }
        };
        self.run_shell(&rendered, cwd.as_deref()).await?;
        Ok(())
    }
}

/// Extract the signal + optional workspace-path pair that a shell action
/// should use when processing `event`.
fn extract_context(event: &HookEvent) -> (Option<&Signal>, Option<PathBuf>) {
    match event {
        HookEvent::SignalReceived { signal } | HookEvent::WorkspaceSetupStarting { signal } => {
            (Some(signal.as_signal()), None)
        }
        HookEvent::WorkspaceSetupFinished { signal, path }
        | HookEvent::AgentStarting { signal, path, .. }
        | HookEvent::AgentFinished { signal, path, .. }
        | HookEvent::WorkspaceTeardownStarting { signal, path }
        | HookEvent::WorkspaceTeardownFinished { signal, path } => {
            (Some(signal.as_signal()), Some(path.clone()))
        }
        HookEvent::DequeueFailed { .. }
        | HookEvent::RenderPromptFailed { .. }
        | HookEvent::WorkspaceSetupFailed { .. }
        | HookEvent::AgentRunFailed { .. }
        | HookEvent::WorkspaceTeardownFailed { .. }
        | HookEvent::RunnerStarting {}
        | HookEvent::RunnerFinished { .. } => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::{EventDispatcher, EventName, Metadata, MetadataKey, MetadataValue, Signal};

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    fn empty_signal() -> Signal {
        Signal::new(Metadata::new())
    }

    fn torndown_event(path: PathBuf) -> HookEvent {
        HookEvent::WorkspaceTeardownFinished {
            signal: empty_signal().into(),
            path,
        }
    }

    #[tokio::test]
    async fn shell_action_only_runs_on_registered_event() {
        let action = ShellAction::new("true").expect("compile");
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(EventName::AgentFinished, action);

        let report = dispatcher
            .emit(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await;
        assert!(report.is_clean());
    }

    #[tokio::test]
    async fn shell_action_logs_but_does_not_propagate_nonzero_exit() {
        let action = ShellAction::new("false").expect("compile");
        action
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("must not propagate");
    }

    #[tokio::test]
    async fn shell_action_renders_signal_and_metadata_templates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("file").expect("key"),
            MetadataValue::String("src/lib.rs".into()),
        );
        let signal = Signal::new(metadata);
        let signal_id = signal.id().to_string();

        let action =
            ShellAction::new("echo {{metadata.file}}:{{signal.id}} > marker.txt").expect("compile");
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: signal.into(),
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("action ok");

        let marker = ws.join("marker.txt");
        let contents = std::fs::read_to_string(&marker).expect("marker");
        assert!(
            contents.contains("src/lib.rs"),
            "metadata not rendered: {contents:?}"
        );
        assert!(
            contents.contains(&signal_id),
            "signal.id not rendered: {contents:?}"
        );
    }

    #[tokio::test]
    async fn shell_action_renders_iteration_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();
        let signal = Signal::new(Metadata::new());

        let action = ShellAction::new(
            "echo n={{iteration.count}} prev={{iteration.previous_result}} > iter.txt",
        )
        .expect("compile");
        let iteration = IterationContext::for_count(7);
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: signal.into(),
                    path: ws.clone(),
                },
                &iteration,
            )
            .await
            .expect("action ok");

        let contents = std::fs::read_to_string(ws.join("iter.txt")).expect("iter.txt");
        assert!(
            contents.contains("n=7"),
            "iteration.count missing: {contents:?}"
        );
        assert!(
            contents.contains("prev=none"),
            "iteration.previous_result missing: {contents:?}"
        );
    }

    #[tokio::test]
    async fn shell_action_lifecycle_event_renders_iteration_only() {
        let action = ShellAction::new("true {{iteration.count}} {{today}}").expect("compile");
        action
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("lifecycle action ok");
    }

    #[tokio::test]
    async fn shell_action_lifecycle_event_with_signal_root_is_swallowed() {
        let action = ShellAction::new("echo {{signal.id}}").expect("compile");
        action
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_action_template_error_is_logged_not_propagated() {
        let action = ShellAction::new("echo {{metadata.nonexistent}}").expect("compile");
        action
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_action_uses_workspace_path_as_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let action = ShellAction::new("pwd > pwd.txt").expect("compile");
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: empty_signal().into(),
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("action ok");

        let pwd_contents = std::fs::read_to_string(ws.join("pwd.txt")).expect("pwd file");
        let observed = PathBuf::from(pwd_contents.trim());
        let expected = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
        let observed_canon = std::fs::canonicalize(&observed).unwrap_or_else(|_| observed.clone());
        assert_eq!(observed_canon, expected);
    }

    #[tokio::test]
    async fn extract_context_returns_none_for_runner_lifecycle_events() {
        let (signal, cwd) = extract_context(&HookEvent::RunnerStarting {});
        assert!(signal.is_none());
        assert!(cwd.is_none());

        let (signal, cwd) = extract_context(&HookEvent::RunnerFinished {
            reason: iter_core::RunnerTerminationReason::Once,
            iteration_count: 0,
        });
        assert!(signal.is_none());
        assert!(cwd.is_none());
    }
}
