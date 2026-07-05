//! Typed value enums and selectors for `cursor-agent`'s value-taking flags.

/// `--output-format <format>` (only meaningful with `--print`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `text` — the CLI default: human-readable console output.
    Text,
    /// `json` — a single terminal `result` JSON object.
    Json,
    /// `stream-json` — a newline-delimited event stream ending in the
    /// terminal `result` record.
    StreamJson,
}

impl OutputFormat {
    /// The value as `cursor-agent`'s own lowercase flag token.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// `--mode <mode>` — the non-default execution modes.
///
/// The default (unset) mode is the full agent; `plan` and `ask` are the two
/// read-only variants the CLI exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecutionMode {
    /// `plan` — read-only/planning (analyze, propose plans, no edits).
    Plan,
    /// `ask` — Q&A style for explanations and questions (read-only).
    Ask,
}

impl ExecutionMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Ask => "ask",
        }
    }
}

/// `--sandbox <mode>` — explicitly override the configured sandbox state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SandboxMode {
    /// `enabled`.
    Enabled,
    /// `disabled`.
    Disabled,
}

impl SandboxMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// `--resume [chatId]` — an optional-value flag.
///
/// `cursor-agent` accepts `--resume` with no value (opening a session picker)
/// or `--resume <chatId>` to target a specific chat.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeSelector {
    /// `--resume` with no chat id: select a session to resume.
    Prompt,
    /// `--resume <chatId>`: resume the named chat.
    Chat(String),
}

/// `-w, --worktree [name]` — an optional-value flag.
///
/// `cursor-agent` accepts `--worktree` with no value (a name is generated) or
/// `--worktree <name>` to pin the isolated worktree's name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Worktree {
    /// `--worktree` with no name: the CLI generates one.
    Auto,
    /// `--worktree <name>`: use the given worktree name.
    Named(String),
}
