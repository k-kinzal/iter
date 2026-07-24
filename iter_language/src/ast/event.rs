//! Event-handler AST for Runner and Compose hooks.

use super::Expr;

/// A Runner-scoped `on <event-name> { <actions> }` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventHandlerDef {
    /// Lifecycle event the handler subscribes to.
    pub event: EventName,
    /// Optional boolean expression evaluated before the handler's actions.
    pub condition: Option<Expr>,
    /// Actions to execute, in source order.
    pub actions: Vec<RunnerAction>,
}

/// A Compose-level `on <event-name> { <selectors> <actions> }` declaration.
///
/// Compose hooks belong to the orchestrator declaration, not to a Runner.
/// Resource selectors are optional: `None` means every resource of the
/// corresponding kind managed by this Compose run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeHookDef {
    /// Compose lifecycle event the hook subscribes to.
    pub event: ComposeEventName,
    /// Service filter or aggregate target set.
    pub services: Option<Vec<String>>,
    /// Trigger filter or aggregate target set.
    pub triggers: Option<Vec<String>>,
    /// Actions to execute, in source order.
    pub actions: Vec<ComposeAction>,
}

/// Lifecycle and aggregate events observable by a Compose orchestrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComposeEventName {
    /// Before the first managed resource is started.
    ComposeStarting,
    /// After every initial managed resource has reached its running state.
    ComposeStarted,
    /// After every managed task completed normally, before queues are closed.
    ComposeCompleting,
    /// After normal shutdown and queue close complete.
    ComposeCompleted,
    /// When the first fatal managed-resource failure is known, before policy.
    ComposeFailing,
    /// After failure handling, resource stop, and queue close complete.
    ComposeFailed,
    /// Before externally requested resource shutdown begins.
    ComposeStopping,
    /// After externally requested resource shutdown and queue close complete.
    ComposeStopped,
    /// Immediately before a service's iter process is started.
    ServiceStarting,
    /// After a service's iter process reaches `Running`.
    ServiceStarted,
    /// After a service's iter process stops normally.
    ServiceCompleted,
    /// After a service fails to start, run, monitor, or finalize.
    ServiceFailed,
    /// After a service is killed by an external stop or orchestrator cancellation.
    ServiceKilled,
    /// Immediately before a Trigger supervisor is started.
    TriggerStarting,
    /// After a Trigger reaches `Running`, including after restarts.
    TriggerStarted,
    /// After a Trigger supervisor enters restart backoff.
    TriggerRestarting,
    /// After a finite Trigger completes normally.
    TriggerCompleted,
    /// After a Trigger becomes permanently failed.
    TriggerFailed,
    /// After a Trigger is stopped by orchestrator shutdown.
    TriggerStopped,
    /// Once every selected service has reached `Running` at least once.
    ServicesStarted,
    /// Once every selected service has stopped normally.
    ServicesCompleted,
    /// Once the first selected service fails.
    ServicesFailed,
    /// Once every selected service is completed, failed, or killed.
    ServicesSettled,
    /// Once every selected Trigger has reached `Running` at least once.
    TriggersStarted,
    /// Once every selected finite Trigger has completed normally.
    TriggersCompleted,
    /// Once the first selected Trigger becomes permanently failed.
    TriggersFailed,
    /// Once every selected Trigger is completed, failed, or stopped.
    TriggersSettled,
}

impl ComposeEventName {
    /// Return the canonical source-form spelling for this event.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ComposeStarting => "compose_starting",
            Self::ComposeStarted => "compose_started",
            Self::ComposeCompleting => "compose_completing",
            Self::ComposeCompleted => "compose_completed",
            Self::ComposeFailing => "compose_failing",
            Self::ComposeFailed => "compose_failed",
            Self::ComposeStopping => "compose_stopping",
            Self::ComposeStopped => "compose_stopped",
            Self::ServiceStarting => "service_starting",
            Self::ServiceStarted => "service_started",
            Self::ServiceCompleted => "service_completed",
            Self::ServiceFailed => "service_failed",
            Self::ServiceKilled => "service_killed",
            Self::TriggerStarting => "trigger_starting",
            Self::TriggerStarted => "trigger_started",
            Self::TriggerRestarting => "trigger_restarting",
            Self::TriggerCompleted => "trigger_completed",
            Self::TriggerFailed => "trigger_failed",
            Self::TriggerStopped => "trigger_stopped",
            Self::ServicesStarted => "services_started",
            Self::ServicesCompleted => "services_completed",
            Self::ServicesFailed => "services_failed",
            Self::ServicesSettled => "services_settled",
            Self::TriggersStarted => "triggers_started",
            Self::TriggersCompleted => "triggers_completed",
            Self::TriggersFailed => "triggers_failed",
            Self::TriggersSettled => "triggers_settled",
        }
    }

    /// Parse a canonical source-form spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "compose_starting" => Self::ComposeStarting,
            "compose_started" => Self::ComposeStarted,
            "compose_completing" => Self::ComposeCompleting,
            "compose_completed" => Self::ComposeCompleted,
            "compose_failing" => Self::ComposeFailing,
            "compose_failed" => Self::ComposeFailed,
            "compose_stopping" => Self::ComposeStopping,
            "compose_stopped" => Self::ComposeStopped,
            "service_starting" => Self::ServiceStarting,
            "service_started" => Self::ServiceStarted,
            "service_completed" => Self::ServiceCompleted,
            "service_failed" => Self::ServiceFailed,
            "service_killed" => Self::ServiceKilled,
            "trigger_starting" => Self::TriggerStarting,
            "trigger_started" => Self::TriggerStarted,
            "trigger_restarting" => Self::TriggerRestarting,
            "trigger_completed" => Self::TriggerCompleted,
            "trigger_failed" => Self::TriggerFailed,
            "trigger_stopped" => Self::TriggerStopped,
            "services_started" => Self::ServicesStarted,
            "services_completed" => Self::ServicesCompleted,
            "services_failed" => Self::ServicesFailed,
            "services_settled" => Self::ServicesSettled,
            "triggers_started" => Self::TriggersStarted,
            "triggers_completed" => Self::TriggersCompleted,
            "triggers_failed" => Self::TriggersFailed,
            "triggers_settled" => Self::TriggersSettled,
            _ => return None,
        })
    }

    /// Whether this event is scoped to one or more services.
    #[must_use]
    pub fn uses_services(self) -> bool {
        matches!(
            self,
            Self::ServiceStarting
                | Self::ServiceStarted
                | Self::ServiceCompleted
                | Self::ServiceFailed
                | Self::ServiceKilled
                | Self::ServicesStarted
                | Self::ServicesCompleted
                | Self::ServicesFailed
                | Self::ServicesSettled
        )
    }

    /// Whether this event is scoped to one or more Triggers.
    #[must_use]
    pub fn uses_triggers(self) -> bool {
        matches!(
            self,
            Self::TriggerStarting
                | Self::TriggerStarted
                | Self::TriggerRestarting
                | Self::TriggerCompleted
                | Self::TriggerFailed
                | Self::TriggerStopped
                | Self::TriggersStarted
                | Self::TriggersCompleted
                | Self::TriggersFailed
                | Self::TriggersSettled
        )
    }

    /// Whether this is a handler-local aggregate event.
    #[must_use]
    pub fn is_aggregate(self) -> bool {
        matches!(
            self,
            Self::ServicesStarted
                | Self::ServicesCompleted
                | Self::ServicesFailed
                | Self::ServicesSettled
                | Self::TriggersStarted
                | Self::TriggersCompleted
                | Self::TriggersFailed
                | Self::TriggersSettled
        )
    }

    /// All known canonical Compose event names.
    pub const ALL: &'static [&'static str] = &[
        "compose_starting",
        "compose_started",
        "compose_completing",
        "compose_completed",
        "compose_failing",
        "compose_failed",
        "compose_stopping",
        "compose_stopped",
        "service_starting",
        "service_started",
        "service_completed",
        "service_failed",
        "service_killed",
        "trigger_starting",
        "trigger_started",
        "trigger_restarting",
        "trigger_completed",
        "trigger_failed",
        "trigger_stopped",
        "services_started",
        "services_completed",
        "services_failed",
        "services_settled",
        "triggers_started",
        "triggers_completed",
        "triggers_failed",
        "triggers_settled",
    ];
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
    /// `runner_completing` — a condition requested completion and the
    /// current iteration has settled.
    RunnerCompleting,
    /// `runner_completed` — finalization succeeded and the outcome is durable.
    RunnerCompleted,
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
            EventName::RunnerCompleting => "runner_completing",
            EventName::RunnerCompleted => "runner_completed",
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
            "runner_completing" => (EventName::RunnerCompleting, None),
            "runner_completed" => (EventName::RunnerCompleted, None),
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
        "runner_completing",
        "runner_completed",
        "runner_finished",
    ];
}

/// A shell action executed by a Runner or Compose lifecycle hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellActionDef {
    /// Shell source rendered immediately before execution.
    pub script: String,
    /// Ordered stream captures published after the process exits.
    pub captures: Vec<ShellCaptureDef>,
}

/// A Signal publication requested by a lifecycle hook.
///
/// Compose Hooks resolve `target` against the flattened Compose queue set.
/// `None` selects the only declared queue and is rejected when resolution is
/// ambiguous. Runner Hooks do not accept this action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueActionDef {
    /// Named destination queue, or implicit single-queue resolution.
    pub target: Option<String>,
    /// Metadata template fields rendered when the Hook fires.
    pub metadata: Vec<(String, String)>,
    /// Signal priority. `None` means `normal`.
    pub priority: Option<super::PriorityKeyword>,
}

impl ShellActionDef {
    /// Build the backward-compatible `shell "<script>"` form.
    #[must_use]
    pub fn simple(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            captures: Vec::new(),
        }
    }
}

/// One named capture produced by a block-form shell action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCaptureDef {
    /// Name exposed below the `var.*` template root.
    pub name: String,
    /// Process stream to capture.
    pub stream: ShellCaptureStream,
    /// Whether the captured bytes replace or append to the existing value.
    pub mode: ShellCaptureMode,
    /// Structured parsing policy applied after the raw stream is updated.
    pub parse: ShellCaptureParse,
}

/// Process stream selected by a shell capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCaptureStream {
    /// Capture standard output.
    Stdout,
    /// Capture standard error.
    Stderr,
}

/// Update mode for an existing named capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCaptureMode {
    /// Replace the previous raw stream.
    Replace,
    /// Append bytes to the previous raw stream, then parse the whole value.
    Append,
}

/// Parser selection for one shell capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCaptureParse {
    /// Try the conservative built-in sequence, then fall back to text.
    Auto,
    /// Try formats in declaration order.
    Ordered(Vec<ShellCaptureFormat>),
}

/// Formats supported by shell capture decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCaptureFormat {
    /// Raw UTF-8 text.
    Text,
    /// Text split into logical lines.
    Lines,
    /// One JSON value.
    Json,
    /// One JSON value per non-empty line.
    Ndjson,
    /// A YAML document.
    Yaml,
    /// A TOML document.
    Toml,
    /// Comma-separated rows.
    Csv,
    /// Tab-separated rows.
    Tsv,
}

impl ShellCaptureFormat {
    /// Parse a source-form format name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "lines" => Some(Self::Lines),
            "json" => Some(Self::Json),
            "ndjson" => Some(Self::Ndjson),
            "yaml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            _ => None,
        }
    }

    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Lines => "lines",
            Self::Json => "json",
            Self::Ndjson => "ndjson",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
        }
    }

    /// Every accepted source spelling.
    pub const ALL: &'static [&'static str] = &[
        "text", "lines", "json", "ndjson", "yaml", "toml", "csv", "tsv",
    ];
}

/// Action supported by a Runner lifecycle hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerAction {
    /// `shell "<script>"` or block-form `shell { ... }` action.
    Shell(ShellActionDef),
}

/// Action supported by a Compose lifecycle hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeAction {
    /// `shell "<script>"` or block-form `shell { ... }` action.
    Shell(ShellActionDef),
    /// `enqueue { target = <queue> ... }` Signal publication.
    Enqueue(EnqueueActionDef),
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

    #[test]
    fn compose_event_names_round_trip() {
        for name in ComposeEventName::ALL {
            let parsed = ComposeEventName::parse(name).expect("canonical name parses");
            assert_eq!(parsed.as_str(), *name);
        }
    }
}
