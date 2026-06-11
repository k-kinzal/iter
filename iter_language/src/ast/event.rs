//! Event-handler AST: top-level `on <event> { ... }` declarations.

/// A top-level `on <event-name> { <actions> }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventHandlerDef {
    /// Lifecycle event the handler subscribes to.
    pub event: EventName,
    /// Actions to execute, in source order.
    pub actions: Vec<Action>,
}

/// Lifecycle events recognised by the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventName {
    /// `runner_starting` — fired exactly once before the runner enters its
    /// per-signal loop.
    RunnerStarting,
    /// `signal_received` — fired when the runner pulls a signal from the queue.
    SignalReceived,
    /// `workspace_setup_starting` — fired before the workspace is set up.
    WorkspaceSetupStarting,
    /// `workspace_setup_finished` — fired after the workspace is set up.
    WorkspaceSetupFinished,
    /// `agent_starting` — fired immediately before the agent process starts.
    AgentStarting,
    /// `agent_finished` — fired after the agent process exits.
    AgentFinished,
    /// `workspace_teardown_starting` — fired before workspace teardown.
    WorkspaceTeardownStarting,
    /// `workspace_teardown_finished` — fired after workspace teardown.
    WorkspaceTeardownFinished,
    /// `runner_error` — fired when any earlier stage fails.
    RunnerError,
    /// `runner_finished` — fired exactly once just before the runner stops,
    /// regardless of termination reason.
    RunnerFinished,
}

impl EventName {
    /// Return the canonical source-form spelling for this event.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EventName::RunnerStarting => "runner_starting",
            EventName::SignalReceived => "signal_received",
            EventName::WorkspaceSetupStarting => "workspace_setup_starting",
            EventName::WorkspaceSetupFinished => "workspace_setup_finished",
            EventName::AgentStarting => "agent_starting",
            EventName::AgentFinished => "agent_finished",
            EventName::WorkspaceTeardownStarting => "workspace_teardown_starting",
            EventName::WorkspaceTeardownFinished => "workspace_teardown_finished",
            EventName::RunnerError => "runner_error",
            EventName::RunnerFinished => "runner_finished",
        }
    }

    /// Parse a source-form spelling. Unknown names return `None`.
    ///
    /// Accepts both the canonical spelling and historical aliases (e.g.
    /// `workspace_setting_up` for [`EventName::WorkspaceSetupStarting`]).
    /// Callers that need to know whether an alias was used should call
    /// [`EventName::parse_with_deprecation`] instead.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::parse_with_deprecation(s).map(|(name, _)| name)
    }

    /// Like [`EventName::parse`], but also returns the deprecated alias the
    /// input matched (if any). When the input matched the canonical
    /// spelling the second tuple element is `None`. When it matched a
    /// deprecated alias the second element carries the alias as written
    /// in the source so callers can produce a `did you mean ...?` style
    /// deprecation warning.
    #[must_use]
    pub fn parse_with_deprecation(s: &str) -> Option<(Self, Option<&'static str>)> {
        let (name, alias) = match s {
            // Canonical names.
            "runner_starting" => (EventName::RunnerStarting, None),
            "signal_received" => (EventName::SignalReceived, None),
            "workspace_setup_starting" => (EventName::WorkspaceSetupStarting, None),
            "workspace_setup_finished" => (EventName::WorkspaceSetupFinished, None),
            "agent_starting" => (EventName::AgentStarting, None),
            "agent_finished" => (EventName::AgentFinished, None),
            "workspace_teardown_starting" => (EventName::WorkspaceTeardownStarting, None),
            "workspace_teardown_finished" => (EventName::WorkspaceTeardownFinished, None),
            "runner_error" => (EventName::RunnerError, None),
            "runner_finished" => (EventName::RunnerFinished, None),

            // Deprecated aliases.
            "workspace_setting_up" => (
                EventName::WorkspaceSetupStarting,
                Some("workspace_setting_up"),
            ),
            "workspace_set_up" => (EventName::WorkspaceSetupFinished, Some("workspace_set_up")),
            "workspace_tearing_down" => (
                EventName::WorkspaceTeardownStarting,
                Some("workspace_tearing_down"),
            ),
            "workspace_torndown" => (
                EventName::WorkspaceTeardownFinished,
                Some("workspace_torndown"),
            ),

            _ => return None,
        };
        Some((name, alias))
    }

    /// All known event names, canonical spelling only.
    ///
    /// Deprecated aliases are deliberately excluded so spell-check
    /// suggestions never steer users back toward a name we are trying to
    /// retire.
    pub const ALL: &'static [&'static str] = &[
        "runner_starting",
        "signal_received",
        "workspace_setup_starting",
        "workspace_setup_finished",
        "agent_starting",
        "agent_finished",
        "workspace_teardown_starting",
        "workspace_teardown_finished",
        "runner_error",
        "runner_finished",
    ];
}

/// Action to perform when a top-level event handler fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `shell "<command>"` action.
    Shell(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_canonical_names() {
        for name in EventName::ALL {
            let parsed = EventName::parse(name).expect("canonical name parses");
            assert_eq!(parsed.as_str(), *name, "round-trip {name}");
        }
    }

    #[test]
    fn parse_with_deprecation_returns_none_alias_for_canonical_names() {
        for name in EventName::ALL {
            let (_, alias) = EventName::parse_with_deprecation(name).expect("canonical parses");
            assert!(alias.is_none(), "{name} is canonical, must not flag alias");
        }
    }

    #[test]
    fn parse_accepts_deprecated_aliases() {
        let cases = [
            ("workspace_setting_up", EventName::WorkspaceSetupStarting),
            ("workspace_set_up", EventName::WorkspaceSetupFinished),
            (
                "workspace_tearing_down",
                EventName::WorkspaceTeardownStarting,
            ),
            ("workspace_torndown", EventName::WorkspaceTeardownFinished),
        ];
        for (alias, expected) in cases {
            let (parsed, deprecated) = EventName::parse_with_deprecation(alias)
                .unwrap_or_else(|| panic!("alias `{alias}` should parse"));
            assert_eq!(parsed, expected, "alias `{alias}` resolves to canonical");
            assert_eq!(
                deprecated,
                Some(alias),
                "alias `{alias}` flagged as deprecated"
            );
        }
    }

    #[test]
    fn parse_rejects_unknown_names() {
        assert!(EventName::parse("not_an_event").is_none());
        assert!(EventName::parse_with_deprecation("not_an_event").is_none());
    }

    #[test]
    fn all_excludes_deprecated_aliases() {
        for name in EventName::ALL {
            assert!(
                !matches!(
                    *name,
                    "workspace_setting_up"
                        | "workspace_set_up"
                        | "workspace_tearing_down"
                        | "workspace_torndown"
                ),
                "deprecated alias `{name}` leaked into EventName::ALL"
            );
        }
    }
}
