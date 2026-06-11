//! [`ShellEventHandler`] — execute `shell "..."` actions for `on <event> {}`
//! handlers.
//!
//! The handler runs a shell command when the emitter dispatches an event
//! it was registered for. Because the [`EventEmitter`](super::EventEmitter)
//! routes by [`EventName`](super::EventName), the handler itself carries
//! no event-name field — it is a pure action callback.
//!
//! # Template rendering
//!
//! The command string is compiled once into a [`Template`] and rendered
//! per-event against a [`RenderContext`] — the same machinery the runner
//! uses for prompts. Template variables include `{{signal.id}}`,
//! `{{signal.created_at}}`, `{{today}}`, every `{{metadata.*}}` key
//! attached to the signal, and the per-turn `{{iteration.*}}` snapshot.
//! Signal-less lifecycle events (`runner_starting`, `runner_finished`,
//! `runner_error` raised before a signal was dequeued) render against a
//! [`LifecycleRenderContext`] so `{{signal.*}}` and `{{metadata.*}}` are
//! deliberately absent.
//!
//! # Working directory
//!
//! When the triggering event carries a workspace path (everything after
//! `workspace_setup_finished`), the shell command runs with that path as
//! its cwd. Events without a workspace path (`runner_starting`,
//! `runner_finished`, `signal_received`, `workspace_setup_starting`,
//! `runner_error`) inherit the parent's cwd.
//!
//! Shell commands run via `sh -c <cmd>` and inherit the parent's stdio.
//! A non-zero exit status is *logged* but never propagated back to the
//! runner — the [`EventEmitter`](super::EventEmitter) contract calls
//! event handlers on a best-effort basis.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::warn;

use super::event::Event;
use super::event_handler::{BoxError, EventHandler};
use super::iteration::IterationContext;
use crate::signal::Signal;
use crate::template::{LifecycleRenderContext, RenderContext, Template, TemplateError};

/// Event handler that runs a shell command.
///
/// The handler holds only the action — the compiled command template and
/// execution logic. Which event it handles is the emitter's
/// responsibility at registration time.
#[derive(Debug, Clone)]
pub struct ShellEventHandler {
    command_source: String,
    compiled: Template,
}

impl ShellEventHandler {
    /// Build a handler that runs `command` when invoked.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] if `command` is not a
    /// valid Handlebars template.
    pub fn new(command: impl Into<String>) -> Result<Self, TemplateError> {
        let command_source = command.into();
        let compiled = Template::compile(command_source.clone())?;
        Ok(Self {
            command_source,
            compiled,
        })
    }

    /// The shell command template this handler will render and run.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command_source
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
                "shell event handler exited non-zero"
            );
        }
        Ok(())
    }
}

impl EventHandler for ShellEventHandler {
    async fn handle(&self, event: &Event, iteration: &IterationContext) -> Result<(), BoxError> {
        let (signal, cwd) = extract_context(event);
        let render_result = match signal {
            Some(signal) => {
                let ctx = RenderContext::new(signal, iteration);
                self.compiled.render(&ctx)
            }
            None => self
                .compiled
                .render(&LifecycleRenderContext::new(iteration)),
        };
        let rendered = match render_result {
            Ok(text) => text,
            Err(err) => {
                warn!(
                    command = %self.command_source,
                    error = %err,
                    "shell event handler template render failed"
                );
                return Ok(());
            }
        };
        self.run_shell(&rendered, cwd.as_deref()).await?;
        Ok(())
    }
}

/// Extract the signal + optional workspace-path pair that a shell handler
/// should use when processing `event`.
fn extract_context(event: &Event) -> (Option<&Signal>, Option<PathBuf>) {
    match event {
        Event::SignalReceived { signal } | Event::WorkspaceSetupStarting { signal } => {
            (Some(signal), None)
        }
        Event::WorkspaceSetupFinished { signal, path }
        | Event::AgentStarting { signal, path, .. }
        | Event::AgentFinished { signal, path, .. }
        | Event::WorkspaceTeardownStarting { signal, path }
        | Event::WorkspaceTeardownFinished { signal, path } => (Some(signal), Some(path.clone())),
        Event::DequeueFailed { .. }
        | Event::RenderPromptFailed { .. }
        | Event::WorkspaceSetupFailed { .. }
        | Event::AgentRunFailed { .. }
        | Event::WorkspaceTeardownFailed { .. }
        | Event::RunnerStarting {}
        | Event::RunnerFinished { .. } => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::event::EventName;
    use crate::runner::event_emitter::EventEmitter;
    use crate::signal::{Metadata, MetadataKey, MetadataValue, Signal};

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    fn empty_signal() -> Signal {
        Signal::new(Metadata::new())
    }

    fn torndown_event(path: PathBuf) -> Event {
        Event::WorkspaceTeardownFinished {
            signal: empty_signal(),
            path,
        }
    }

    #[tokio::test]
    async fn shell_handler_only_runs_on_registered_event() {
        let handler = ShellEventHandler::new("true").expect("compile");
        let mut emitter = EventEmitter::new();
        emitter.on(EventName::AgentFinished, handler);

        let report = emitter
            .emit(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await;
        assert!(report.is_clean());
    }

    #[tokio::test]
    async fn shell_handler_logs_but_does_not_propagate_nonzero_exit() {
        let handler = ShellEventHandler::new("false").expect("compile");
        handler
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("must not propagate");
    }

    #[tokio::test]
    async fn shell_handler_renders_signal_and_metadata_templates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("file").expect("key"),
            MetadataValue::String("src/lib.rs".into()),
        );
        let signal = Signal::new(metadata);
        let signal_id = signal.id().to_string();

        let handler = ShellEventHandler::new("echo {{metadata.file}}:{{signal.id}} > marker.txt")
            .expect("compile");
        handler
            .handle(
                &Event::WorkspaceTeardownFinished {
                    signal,
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("handler ok");

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
    async fn shell_handler_renders_iteration_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();
        let signal = Signal::new(Metadata::new());

        let handler = ShellEventHandler::new(
            "echo n={{iteration.count}} prev={{iteration.previous_result}} > iter.txt",
        )
        .expect("compile");
        let iteration = IterationContext::for_count(7);
        handler
            .handle(
                &Event::WorkspaceTeardownFinished {
                    signal,
                    path: ws.clone(),
                },
                &iteration,
            )
            .await
            .expect("handler ok");

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
    async fn shell_handler_lifecycle_event_renders_iteration_only() {
        let handler =
            ShellEventHandler::new("true {{iteration.count}} {{today}}").expect("compile");
        handler
            .handle(&Event::RunnerStarting {}, &iter_ctx())
            .await
            .expect("lifecycle handler ok");
    }

    #[tokio::test]
    async fn shell_handler_lifecycle_event_with_signal_root_is_swallowed() {
        let handler = ShellEventHandler::new("echo {{signal.id}}").expect("compile");
        handler
            .handle(&Event::RunnerStarting {}, &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_handler_template_error_is_logged_not_propagated() {
        let handler = ShellEventHandler::new("echo {{metadata.nonexistent}}").expect("compile");
        handler
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_handler_uses_workspace_path_as_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let handler = ShellEventHandler::new("pwd > pwd.txt").expect("compile");
        handler
            .handle(
                &Event::WorkspaceTeardownFinished {
                    signal: empty_signal(),
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("handler ok");

        let pwd_contents = std::fs::read_to_string(ws.join("pwd.txt")).expect("pwd file");
        let observed = PathBuf::from(pwd_contents.trim());
        let expected = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
        let observed_canon = std::fs::canonicalize(&observed).unwrap_or_else(|_| observed.clone());
        assert_eq!(observed_canon, expected);
    }

    #[tokio::test]
    async fn extract_context_returns_none_for_runner_lifecycle_events() {
        let (signal, cwd) = extract_context(&Event::RunnerStarting {});
        assert!(signal.is_none());
        assert!(cwd.is_none());

        let (signal, cwd) = extract_context(&Event::RunnerFinished {
            reason: crate::runner::config::RunnerTerminationReason::Once,
            iteration_count: 0,
        });
        assert!(signal.is_none());
        assert!(cwd.is_none());
    }
}
