//! Event-handler registration — wires `on <event> { shell "..." }` blocks
//! from the Iterfile into the [`RunnerBuilder`].
//!
//! [`ShellEventHandler`] lives in [`iter_core`]; this module provides the
//! registration functions that translate language-level declarations into
//! core-level handler registrations.

use iter_core::{EventName, RunnerBuilder, ShellEventHandler, TemplateError};
use iter_language::{Action, EventHandlerDef, Iterfile, Spanned};

use crate::{AnyAgent, AnyWorkspace};

/// Map a language-level [`iter_language::EventName`] to the core-level
/// [`iter_core::EventName`] routing key.
fn to_core_event_name(name: iter_language::EventName) -> EventName {
    match name {
        iter_language::EventName::RunnerStarting => EventName::RunnerStarting,
        iter_language::EventName::SignalReceived => EventName::SignalReceived,
        iter_language::EventName::WorkspaceSetupStarting => EventName::WorkspaceSetupStarting,
        iter_language::EventName::WorkspaceSetupFinished => EventName::WorkspaceSetupFinished,
        iter_language::EventName::AgentStarting => EventName::AgentStarting,
        iter_language::EventName::AgentFinished => EventName::AgentFinished,
        iter_language::EventName::WorkspaceTeardownStarting => EventName::WorkspaceTeardownStarting,
        iter_language::EventName::WorkspaceTeardownFinished => EventName::WorkspaceTeardownFinished,
        iter_language::EventName::RunnerError => EventName::RunnerError,
        iter_language::EventName::RunnerFinished => EventName::RunnerFinished,
    }
}

/// Register every `on <event> { shell "..." }` block from `iterfile` against
/// `builder`.
///
/// Collects event handlers from all runners in the Iterfile and registers them.
/// Convenience wrapper around [`register_event_handlers_from_events`] for the
/// Iterfile case. Compose-side code that ships its own event slice should
/// call [`register_event_handlers_from_events`] directly.
///
/// # Errors
///
/// Returns [`TemplateError`] when any `shell` action fails to compile as a
/// Handlebars template.
pub fn register_event_handlers(
    mut builder: RunnerBuilder<AnyWorkspace, AnyAgent>,
    iterfile: &Iterfile,
) -> Result<RunnerBuilder<AnyWorkspace, AnyAgent>, TemplateError> {
    for runner in &iterfile.runners {
        builder = register_event_handlers_from_events(builder, &runner.node.events)?;
    }
    Ok(builder)
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
    mut builder: RunnerBuilder<AnyWorkspace, AnyAgent>,
    events: &[Spanned<EventHandlerDef>],
) -> Result<RunnerBuilder<AnyWorkspace, AnyAgent>, TemplateError> {
    for spanned in events {
        let Spanned { node, .. } = spanned;
        let EventHandlerDef { event, actions } = node;
        let core_name = to_core_event_name(*event);
        for action in actions {
            match action {
                Action::Shell(cmd) => {
                    let handler = ShellEventHandler::new(cmd.clone())?;
                    builder = builder.on(core_name, handler);
                }
            }
        }
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_language::{Action, EventHandlerDef, Spanned};

    fn handler_decl(event: iter_language::EventName, cmd: &str) -> Spanned<EventHandlerDef> {
        Spanned::new(
            EventHandlerDef {
                event,
                actions: vec![Action::Shell(cmd.to_owned())],
            },
            0..0,
        )
    }

    #[test]
    fn to_core_event_name_maps_all_variants() {
        use iter_language::EventName as Lang;

        let cases = [
            (Lang::RunnerStarting, EventName::RunnerStarting),
            (Lang::SignalReceived, EventName::SignalReceived),
            (
                Lang::WorkspaceSetupStarting,
                EventName::WorkspaceSetupStarting,
            ),
            (
                Lang::WorkspaceSetupFinished,
                EventName::WorkspaceSetupFinished,
            ),
            (Lang::AgentStarting, EventName::AgentStarting),
            (Lang::AgentFinished, EventName::AgentFinished),
            (
                Lang::WorkspaceTeardownStarting,
                EventName::WorkspaceTeardownStarting,
            ),
            (
                Lang::WorkspaceTeardownFinished,
                EventName::WorkspaceTeardownFinished,
            ),
            (Lang::RunnerError, EventName::RunnerError),
            (Lang::RunnerFinished, EventName::RunnerFinished),
        ];
        for (lang, expected) in cases {
            assert_eq!(to_core_event_name(lang), expected, "{lang:?}");
        }
    }

    #[test]
    fn register_rejects_invalid_template() {
        let events = vec![handler_decl(
            iter_language::EventName::RunnerStarting,
            "echo {{",
        )];
        let builder = iter_core::Runner::<AnyWorkspace, AnyAgent>::builder();
        let result = register_event_handlers_from_events(builder, &events);
        assert!(result.is_err());
    }
}
