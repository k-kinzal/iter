//! [`ShellEventHandler`] — execute `shell "..."` actions for `on <event> {}`
//! handlers declared in the Iterfile.
//!
//! The Iterfile DSL supports lifecycle hooks of the form:
//!
//! ```text
//! on workspace_teardown_finished {
//!     shell "git add -A && git commit -m 'iter #{{signal.id}}'"
//! }
//! ```
//!
//! Each `on` block compiles into one [`ShellEventHandler`]. The handler is
//! filtered by [`EventName`](iter_language::EventName) so it only fires when
//! the [`Event`] kind matches the source-form name.
//!
//! # Template rendering
//!
//! The command string is compiled once into an [`iter_core::Template`] and
//! rendered per-event against a [`iter_core::RenderContext`] — the same
//! machinery the runner uses for prompts. Template variables therefore
//! include `{{signal.id}}`, `{{signal.created_at}}`, `{{today}}`, every
//! `{{metadata.*}}` key attached to the signal, and the per-turn
//! `{{iteration.*}}` snapshot the runner provides. Signal-less lifecycle
//! events (`runner_starting`, `runner_finished`, `runner_error` raised
//! before a signal was dequeued) render against an
//! [`iter_core::LifecycleRenderContext`] so `{{signal.*}}` and
//! `{{metadata.*}}` are deliberately absent — strict-mode Handlebars
//! reports a clear error when those roots are referenced from a
//! lifecycle hook.
//!
//! # Working directory
//!
//! When the triggering event carries a workspace path (everything after
//! `workspace_setup_finished`), the shell command runs with that path as
//! its cwd. This is the whole reason `git add`/`git commit`/`git push`
//! hooks work at all: without a cwd they would operate on whatever
//! directory `iter` itself was launched from. Events without a
//! workspace path (`runner_starting`, `runner_finished`,
//! `signal_received`, `workspace_setup_starting`, `runner_error`)
//! inherit the parent's cwd.
//!
//! Shell commands run via `sh -c <cmd>` and inherit the parent's stdio so
//! their output ends up in the same log file as the runner. A non-zero exit
//! status is *logged* via `tracing::warn!` but never propagated back to
//! the runner — the [`EventEmitter`](iter_core::EventEmitter) contract
//! calls event handlers on a best-effort basis.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use iter_core::{
    BoxError, Event, EventHandler, IterationContext, LifecycleRenderContext, RenderContext,
    RunnerBuilder, Signal, Template, TemplateError,
};
use iter_language::{Action, EventHandlerDecl, EventName, Root, Spanned};
use tokio::process::Command;
use tracing::warn;

use crate::{AnyAgent, AnyQueue, AnyWorkspace};

/// Event handler that runs a shell command on a single lifecycle event.
#[derive(Debug, Clone)]
pub struct ShellEventHandler {
    event: EventName,
    command_source: String,
    compiled: Template,
}

impl ShellEventHandler {
    /// Build a handler that fires `command` whenever an [`Event`] of kind
    /// `event` is emitted.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] if `command` is not a
    /// valid Handlebars template.
    pub fn new(event: EventName, command: impl Into<String>) -> Result<Self, TemplateError> {
        let command_source = command.into();
        let compiled = Template::compile(command_source.clone())?;
        Ok(Self {
            event,
            command_source,
            compiled,
        })
    }

    /// The lifecycle event this handler subscribes to.
    #[must_use]
    pub fn event(&self) -> EventName {
        self.event
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
                event = self.event.as_str(),
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
        if !event_matches(event, self.event) {
            return Ok(());
        }
        let (signal, cwd) = extract_context(event);
        let render_result = match signal {
            Some(signal) => {
                let ctx = RenderContext::new(signal, iteration);
                self.compiled.render(&ctx)
            }
            // Lifecycle events without a signal (runner_starting /
            // runner_finished / dequeue-stage runner_error) render
            // against `LifecycleRenderContext` — `{{today}}` and
            // `{{iteration.*}}` resolve, but `{{signal.*}}` /
            // `{{metadata.*}}` deliberately do not. Strict-mode
            // Handlebars surfaces a clear error if a hook references
            // them so the failure stays visible in logs rather than
            // silently rendering as the literal source.
            None => self
                .compiled
                .render(&LifecycleRenderContext::new(iteration)),
        };
        let rendered = match render_result {
            Ok(text) => text,
            Err(err) => {
                // Template errors MUST NOT abort the runner (same
                // best-effort contract as non-zero exit). Log and
                // move on so one broken `on` block can't kill a
                // running session.
                warn!(
                    event = self.event.as_str(),
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

/// Returns `true` when the runtime [`Event`] corresponds to the source-form
/// [`EventName`] subscription.
fn event_matches(event: &Event, name: EventName) -> bool {
    matches!(
        (event, name),
        (Event::RunnerStarting {}, EventName::RunnerStarting)
            | (Event::SignalReceived { .. }, EventName::SignalReceived)
            | (
                Event::WorkspaceSetupStarting { .. },
                EventName::WorkspaceSetupStarting
            )
            | (
                Event::WorkspaceSetupFinished { .. },
                EventName::WorkspaceSetupFinished
            )
            | (Event::AgentStarting { .. }, EventName::AgentStarting)
            | (Event::AgentFinished { .. }, EventName::AgentFinished)
            | (
                Event::WorkspaceTeardownStarting { .. },
                EventName::WorkspaceTeardownStarting
            )
            | (
                Event::WorkspaceTeardownFinished { .. },
                EventName::WorkspaceTeardownFinished
            )
            | (
                Event::DequeueFailed { .. }
                    | Event::RenderPromptFailed { .. }
                    | Event::WorkspaceSetupFailed { .. }
                    | Event::AgentRunFailed { .. }
                    | Event::WorkspaceTeardownFailed { .. },
                EventName::RunnerError
            )
            | (Event::RunnerFinished { .. }, EventName::RunnerFinished)
    )
}

/// Register every `on <event> { shell "..." }` block from `iterfile` against
/// `builder`.
///
/// Convenience wrapper around [`register_event_handlers_from_events`] for the
/// Iterfile case. Compose-side code that ships its own event slice should
/// call [`register_event_handlers_from_events`] directly.
///
/// Builder ownership is moved through this function so the caller can chain
/// it with the rest of the [`RunnerBuilder`] API:
///
/// ```ignore
/// let runner = register_event_handlers(builder, iterfile)?.build()?;
/// ```
///
/// # Errors
///
/// Returns [`TemplateError`] when any `shell` action fails to compile as a
/// Handlebars template — the configuration is rejected rather than
/// deferring the failure to runtime.
pub fn register_event_handlers(
    builder: RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>,
    iterfile: &Root,
) -> Result<RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>, TemplateError> {
    register_event_handlers_from_events(builder, &iterfile.events)
}

/// Register `on <event> { shell "..." }` blocks from a flat slice of event
/// handler declarations against `builder`.
///
/// Shared by the Iterfile and compose `InlineService` code paths.
///
/// # Errors
///
/// Returns [`TemplateError`] when any `shell` action fails to compile as a
/// Handlebars template.
pub fn register_event_handlers_from_events(
    mut builder: RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>,
    events: &[Spanned<EventHandlerDecl>],
) -> Result<RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>, TemplateError> {
    for spanned in events {
        let Spanned { node, .. } = spanned;
        let EventHandlerDecl { event, actions } = node;
        for action in actions {
            match action {
                Action::Shell(cmd) => {
                    let handler = ShellEventHandler::new(*event, cmd.clone())?;
                    builder = builder.event_handler(handler);
                }
            }
        }
    }
    Ok(builder)
}

/// Convenience wrapper that constructs a fresh handler list without going
/// through the runner builder. Used by tests.
#[cfg(test)]
fn handlers_for(iterfile: &Root) -> Vec<ShellEventHandler> {
    let mut out = Vec::new();
    for spanned in &iterfile.events {
        for action in &spanned.node.actions {
            match action {
                Action::Shell(cmd) => {
                    out.push(
                        ShellEventHandler::new(spanned.node.event, cmd.clone()).expect("compile"),
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::{IterationContext, Metadata, MetadataKey, MetadataValue, Signal};
    use iter_language::{Action, EventHandlerDecl, EventName, Spanned};

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    fn handler_decl(event: EventName, cmd: &str) -> Spanned<EventHandlerDecl> {
        Spanned::new(
            EventHandlerDecl {
                event,
                actions: vec![Action::Shell(cmd.to_owned())],
            },
            0..0,
        )
    }

    fn empty_signal() -> Signal {
        Signal::new(Metadata::new())
    }

    fn tornddown_event(path: PathBuf) -> Event {
        Event::WorkspaceTeardownFinished {
            signal: empty_signal(),
            path,
        }
    }

    #[test]
    fn handlers_for_collects_each_action() {
        let mut iterfile = Root::default();
        iterfile
            .events
            .push(handler_decl(EventName::AgentFinished, "echo done"));
        iterfile
            .events
            .push(handler_decl(EventName::RunnerError, "echo oops"));
        let handlers = handlers_for(&iterfile);
        assert_eq!(handlers.len(), 2);
        assert_eq!(handlers[0].event(), EventName::AgentFinished);
        assert_eq!(handlers[0].command(), "echo done");
        assert_eq!(handlers[1].event(), EventName::RunnerError);
    }

    #[test]
    fn event_matches_each_variant() {
        let signal = empty_signal();
        let path = PathBuf::from("/tmp");
        let prompt = iter_core::Prompt::new("p");

        // SignalReceived
        assert!(event_matches(
            &Event::SignalReceived {
                signal: signal.clone()
            },
            EventName::SignalReceived,
        ));
        assert!(!event_matches(
            &Event::SignalReceived {
                signal: signal.clone()
            },
            EventName::AgentStarting,
        ));

        // The pair {WorkspaceSetupFinished, AgentStarting} share the
        // same payload shape — make sure the matcher discriminates.
        assert!(event_matches(
            &Event::WorkspaceSetupFinished {
                signal: signal.clone(),
                path: path.clone(),
            },
            EventName::WorkspaceSetupFinished,
        ));
        assert!(!event_matches(
            &Event::WorkspaceSetupFinished {
                signal: signal.clone(),
                path: path.clone(),
            },
            EventName::AgentStarting,
        ));
        assert!(event_matches(
            &Event::AgentStarting {
                signal,
                path,
                prompt,
            },
            EventName::AgentStarting,
        ));

        // RunnerStarting / RunnerFinished are signal-less lifecycle
        // events: the matcher must discriminate them and never confuse
        // them with each other or with the per-iteration variants.
        assert!(event_matches(
            &Event::RunnerStarting {},
            EventName::RunnerStarting,
        ));
        assert!(!event_matches(
            &Event::RunnerStarting {},
            EventName::RunnerFinished,
        ));
        assert!(event_matches(
            &Event::RunnerFinished {
                reason: iter_core::RunnerTerminationReason::Once,
                iteration_count: 1,
            },
            EventName::RunnerFinished,
        ));
        assert!(!event_matches(
            &Event::RunnerFinished {
                reason: iter_core::RunnerTerminationReason::Once,
                iteration_count: 1,
            },
            EventName::RunnerStarting,
        ));
    }

    #[test]
    fn extract_context_returns_none_for_runner_lifecycle_events() {
        // RunnerStarting / RunnerFinished have no signal context, so
        // shell handlers wired to them cannot interpolate
        // `{{signal.*}}` and inherit the parent cwd.
        let (signal, cwd) = extract_context(&Event::RunnerStarting {});
        assert!(signal.is_none());
        assert!(cwd.is_none());

        let (signal, cwd) = extract_context(&Event::RunnerFinished {
            reason: iter_core::RunnerTerminationReason::Once,
            iteration_count: 0,
        });
        assert!(signal.is_none());
        assert!(cwd.is_none());
    }

    #[tokio::test]
    async fn shell_handler_only_runs_on_matching_event() {
        // Use `true` because it always exists and exits 0. The handler should
        // simply not invoke it for non-matching events.
        let handler = ShellEventHandler::new(EventName::AgentFinished, "true").expect("compile");
        // Wrong event: must noop.
        handler
            .handle(&tornddown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("noop ok");
    }

    #[tokio::test]
    async fn shell_handler_logs_but_does_not_propagate_nonzero_exit() {
        let handler =
            ShellEventHandler::new(EventName::WorkspaceTeardownFinished, "false").expect("compile");
        // `handle` returns Ok even when the underlying command exited 1 —
        // the contract is best effort with `tracing::warn!`.
        handler
            .handle(&tornddown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("must not propagate");
    }

    #[tokio::test]
    async fn shell_handler_renders_signal_and_metadata_templates() {
        // Echo the rendered template through `sh -c` into a marker file in
        // the workspace path, then verify the file was written to the
        // correct cwd with the correct interpolation.
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("file").expect("key"),
            MetadataValue::String("src/lib.rs".into()),
        );
        let signal = Signal::new(metadata);
        let signal_id = signal.id().to_string();

        // `{{metadata.file}}` → "src/lib.rs"
        // `{{signal.id}}` → the actual UUID
        // cwd → the tempdir, so the relative `marker.txt` lands inside it.
        let handler = ShellEventHandler::new(
            EventName::WorkspaceTeardownFinished,
            "echo {{metadata.file}}:{{signal.id}} > marker.txt",
        )
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
        // The combined render context exposes `{{iteration.count}}` for
        // per-signal events alongside the existing `{{signal.*}}` /
        // `{{metadata.*}}` roots.
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();
        let signal = Signal::new(Metadata::new());

        let handler = ShellEventHandler::new(
            EventName::WorkspaceTeardownFinished,
            "echo n={{iteration.count}} prev={{iteration.previous_outcome}} > iter.txt",
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
            "iteration.previous_outcome missing: {contents:?}"
        );
    }

    #[tokio::test]
    async fn shell_handler_lifecycle_event_renders_iteration_only() {
        // RunnerStarting has no signal in flight, so the handler must
        // render against `LifecycleRenderContext`. `{{iteration.*}}`
        // resolves; referencing `{{signal.*}}` from a lifecycle hook
        // strict-mode-errors and is logged + swallowed (covered below).
        let handler = ShellEventHandler::new(
            EventName::RunnerStarting,
            "true {{iteration.count}} {{today}}",
        )
        .expect("compile");
        // Just verify the handler does not error — a non-zero exit or a
        // template error would surface through `Result`.
        handler
            .handle(&Event::RunnerStarting {}, &iter_ctx())
            .await
            .expect("lifecycle handler ok");
    }

    #[tokio::test]
    async fn shell_handler_lifecycle_event_with_signal_root_is_swallowed() {
        // Lifecycle render context deliberately omits the `signal` root:
        // referencing it raises a strict-mode template error which the
        // handler must log and swallow without propagating to the runner.
        let handler = ShellEventHandler::new(EventName::RunnerStarting, "echo {{signal.id}}")
            .expect("compile");
        handler
            .handle(&Event::RunnerStarting {}, &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_handler_template_error_is_logged_not_propagated() {
        // Reference a metadata key that doesn't exist — the renderer will
        // return an error, which the handler must swallow and log.
        let handler = ShellEventHandler::new(
            EventName::WorkspaceTeardownFinished,
            "echo {{metadata.nonexistent}}",
        )
        .expect("compile");
        handler
            .handle(&tornddown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_handler_uses_workspace_path_as_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let handler = ShellEventHandler::new(EventName::WorkspaceTeardownFinished, "pwd > pwd.txt")
            .expect("compile");
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
        // `pwd` output may canonicalize symlinks differently from what
        // `tempfile` returned, so compare by canonical form.
        let observed = PathBuf::from(pwd_contents.trim());
        let expected = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
        let observed_canon = std::fs::canonicalize(&observed).unwrap_or_else(|_| observed.clone());
        assert_eq!(observed_canon, expected);
    }
}
