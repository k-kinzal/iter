//! Typed value enums for Copilot's value-taking flags.

/// `--output-format <format>` — the machine-readability of the run's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `text` (Copilot's default): human-readable console output.
    Text,
    /// `json`: JSONL, one JSON object per line.
    Json,
}

impl OutputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// `--reasoning-effort <level>` (alias `--effort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReasoningEffort {
    /// `none`.
    None,
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    Xhigh,
    /// `max`.
    Max,
}

impl ReasoningEffort {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// `--mode <mode>` — the initial agent mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// `interactive`.
    Interactive,
    /// `plan`.
    Plan,
    /// `autopilot`.
    Autopilot,
}

impl Mode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Plan => "plan",
            Self::Autopilot => "autopilot",
        }
    }
}

/// `--log-level <level>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogLevel {
    /// `none`.
    None,
    /// `error`.
    Error,
    /// `warning`.
    Warning,
    /// `info`.
    Info,
    /// `debug`.
    Debug,
    /// `all`.
    All,
    /// `default`.
    Default,
}

impl LogLevel {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::All => "all",
            Self::Default => "default",
        }
    }
}

/// On/off value shared by `--mouse`, `--bash-env`, and `--stream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Toggle {
    /// `on`.
    On,
    /// `off`.
    Off,
}

impl Toggle {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// The shell a `copilot completion` script targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shell {
    /// `bash`.
    Bash,
    /// `zsh`.
    Zsh,
    /// `fish`.
    Fish,
}

impl Shell {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }
}

/// `copilot update [channel]` — the update channel positional.
///
/// Copilot accepts only `prerelease`; the stable channel is the default and is
/// selected by omitting the positional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateChannel {
    /// `prerelease`.
    Prerelease,
}

impl UpdateChannel {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Prerelease => "prerelease",
        }
    }
}

/// A `--resume[=value]` or `--connect[=sessionId]` selector.
///
/// The bare flag opens Copilot's interactive picker / default connection; the
/// attached form targets a specific session, task, or name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSelector {
    /// The bare flag (`--resume` / `--connect`): interactive picker / default.
    Prompt,
    /// The attached form (`--resume=<id>`): a specific session/task/name.
    Ref(String),
}

impl SessionSelector {
    /// Build a selector targeting a specific session, task id, or name.
    #[must_use]
    pub fn reference(value: impl Into<String>) -> Self {
        Self::Ref(value.into())
    }
}
