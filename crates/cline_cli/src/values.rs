//! Typed value enums for Cline's value-taking flags.

/// `--thinking <level>` — reasoning-effort level for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThinkingLevel {
    /// `none`.
    None,
    /// `low`.
    Low,
    /// `medium` (Cline's default).
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    Xhigh,
}

impl ThinkingLevel {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }
}

/// `--compaction <mode>` — context-compaction strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionMode {
    /// `agentic`.
    Agentic,
    /// `basic` (Cline's default).
    Basic,
    /// `off`.
    Off,
}

impl CompactionMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Agentic => "agentic",
            Self::Basic => "basic",
            Self::Off => "off",
        }
    }
}

/// `--mode <act|plan>` — the agent execution mode used by `schedule create`
/// and the `connect` channel bridges.
///
/// The root run selects plan mode with the `-p, --plan` boolean flag instead;
/// this enum models the subcommands that take an explicit `--mode` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentMode {
    /// `act` — the default: the agent acts, applying tool calls.
    Act,
    /// `plan` — the agent plans without applying changes.
    Plan,
}

impl AgentMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Act => "act",
            Self::Plan => "plan",
        }
    }
}
