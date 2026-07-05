use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

/// Optional CLI value where the flag may appear without an explicit value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionalValue<T> {
    /// Emit only the flag.
    Present,
    /// Emit `--flag=value`.
    Value(T),
}

/// A CLI switch that is either omitted or emitted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Switch {
    /// Omit the flag.
    #[default]
    Off,
    /// Emit the flag.
    On,
}

impl Switch {
    /// Return `true` when the switch should be emitted.
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

impl From<bool> for Switch {
    fn from(value: bool) -> Self {
        if value { Self::On } else { Self::Off }
    }
}

impl From<Switch> for bool {
    fn from(value: Switch) -> Self {
        value.is_on()
    }
}

impl<T> OptionalValue<T> {
    /// Construct a present flag with no explicit value.
    #[must_use]
    pub const fn present() -> Self {
        Self::Present
    }

    /// Construct a flag with an explicit value.
    #[must_use]
    pub const fn value(value: T) -> Self {
        Self::Value(value)
    }
}

/// `--input-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputFormat {
    /// `text`.
    Text,
    /// `stream-json`.
    StreamJson,
}

impl InputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::StreamJson => "stream-json",
        }
    }
}

/// `--output-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum OutputFormat {
    /// `text`.
    Text,
    /// `json`.
    Json,
    /// `stream-json`.
    StreamJson,
}

impl OutputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// `--permission-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionMode {
    /// `acceptEdits`.
    AcceptEdits,
    /// `auto`.
    Auto,
    /// `bypassPermissions`.
    BypassPermissions,
    /// `default`.
    Default,
    /// `dontAsk`.
    DontAsk,
    /// `plan`.
    Plan,
}

impl PermissionMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::BypassPermissions => "bypassPermissions",
            Self::Default => "default",
            Self::DontAsk => "dontAsk",
            Self::Plan => "plan",
        }
    }
}

/// Effort level for session-oriented commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffortLevel {
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    XHigh,
    /// `max`.
    Max,
}

impl EffortLevel {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Boolean spellings accepted by `--prompt-suggestions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BooleanChoice {
    /// `true`.
    True,
    /// `false`.
    False,
    /// `1`.
    One,
    /// `0`.
    Zero,
    /// `yes`.
    Yes,
    /// `no`.
    No,
    /// `on`.
    On,
    /// `off`.
    Off,
}

impl BooleanChoice {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::One => "1",
            Self::Zero => "0",
            Self::Yes => "yes",
            Self::No => "no",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// `--chrome` / `--no-chrome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Chrome {
    /// `--chrome`.
    Enable,
    /// `--no-chrome`.
    Disable,
}

/// `--tmux` optional mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TmuxMode {
    /// `classic`.
    Classic,
}

impl TmuxMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
        }
    }
}

/// Setting sources accepted by `--setting-sources`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingSource {
    /// `user`.
    User,
    /// `project`.
    Project,
    /// `local`.
    Local,
}

impl SettingSource {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

/// `--tools` value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolSet {
    /// `--tools default`.
    Default,
    /// `--tools ""`.
    None,
    /// Comma-separated tool names.
    Tools(Vec<String>),
}

impl ToolSet {
    #[must_use]
    pub(crate) fn value(&self) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::None => String::new(),
            Self::Tools(tools) => tools.join(","),
        }
    }
}

/// Valid non-negative finite value for `--max-budget-usd`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaxBudgetUsd(f64);

impl MaxBudgetUsd {
    /// Create a budget value.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMaxBudgetUsd`] when `amount` is NaN, infinite, or
    /// negative.
    pub fn new(amount: f64) -> Result<Self, InvalidMaxBudgetUsd> {
        if amount.is_finite() && amount >= 0.0 {
            Ok(Self(amount))
        } else {
            Err(InvalidMaxBudgetUsd { amount })
        }
    }

    #[must_use]
    pub(crate) fn render(self) -> String {
        self.0.to_string()
    }

    /// Raw amount.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Invalid `--max-budget-usd` value.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("max budget must be finite and non-negative, got {amount}")]
pub struct InvalidMaxBudgetUsd {
    amount: f64,
}

/// `--file file_id:relative_path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResource {
    /// File id.
    pub file_id: String,
    /// Destination path relative to the startup directory.
    pub relative_path: PathBuf,
}

impl FileResource {
    /// Create a file resource spec.
    #[must_use]
    pub fn new(file_id: impl Into<String>, relative_path: impl Into<PathBuf>) -> Self {
        Self {
            file_id: file_id.into(),
            relative_path: relative_path.into(),
        }
    }

    #[must_use]
    pub(crate) fn value(&self) -> OsString {
        let mut value = OsString::from(&self.file_id);
        value.push(":");
        value.push(&self.relative_path);
        value
    }
}
