//! Typed value enums for Grok's value-taking flags.

/// `--output-format <OUTPUT_FORMAT>` for headless mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `plain` (Grok's default).
    Plain,
    /// `json` — a single terminal JSON object.
    Json,
    /// `streaming-json` — newline-delimited JSON events.
    StreamingJson,
}

impl OutputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Json => "json",
            Self::StreamingJson => "streaming-json",
        }
    }
}

/// `--permission-mode <MODE>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionMode {
    /// `default`.
    Default,
    /// `acceptEdits`.
    AcceptEdits,
    /// `auto`.
    Auto,
    /// `dontAsk`.
    DontAsk,
    /// `bypassPermissions`.
    BypassPermissions,
    /// `plan`.
    Plan,
}

impl PermissionMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
            Self::Plan => "plan",
        }
    }
}

/// `--effort <LEVEL>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effort {
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

impl Effort {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// `-t, --transport <TRANSPORT>` for `grok mcp add`.
///
/// Grok restricts the transport to a fixed set (`[possible values: stdio, http,
/// sse]`), so an arbitrary string is not a valid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// `stdio` — launch a local process and speak over stdin/stdout.
    Stdio,
    /// `http` — connect to a remote server over streamable HTTP.
    Http,
    /// `sse` — connect to a remote server over Server-Sent Events.
    Sse,
}

impl McpTransport {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

/// `-s, --scope <SCOPE>` for `grok mcp add` / `mcp remove`.
///
/// Grok restricts the scope to a fixed set (`[possible values: user, project]`):
/// which config file to write to or remove from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpScope {
    /// `user` — `~/.grok/config.toml`, available in all projects.
    User,
    /// `project` — `./.grok/config.toml`, shared with the directory.
    Project,
}

impl McpScope {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// The shell a `grok completions` script targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletionShell {
    /// `bash`.
    Bash,
    /// `elvish`.
    Elvish,
    /// `fish`.
    Fish,
    /// `powershell`.
    PowerShell,
    /// `zsh`.
    Zsh,
}

impl CompletionShell {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Elvish => "elvish",
            Self::Fish => "fish",
            Self::PowerShell => "powershell",
            Self::Zsh => "zsh",
        }
    }
}

/// Target of `-r, --resume [<SESSION_ID>]`.
///
/// Grok's resume flag takes an *optional* session id: given an id it resumes
/// that specific session; omitted, it resumes the most recent one for the
/// working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeTarget {
    /// `-r` with no id — resume the most recent session.
    MostRecent,
    /// `-r <SESSION_ID>` — resume a specific session by id.
    Session(String),
}

impl ResumeTarget {
    /// Resume a specific session by id.
    #[must_use]
    pub fn session(id: impl Into<String>) -> Self {
        Self::Session(id.into())
    }
}

/// Target of `-w, --worktree [<WORKTREE>]`.
///
/// The worktree flag takes an *optional* name: given a name it starts the
/// session in a worktree with that name; omitted, Grok picks one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Worktree {
    /// `-w` with no name — Grok names the worktree.
    Auto,
    /// `-w <WORKTREE>` — a named worktree.
    Named(String),
}

impl Worktree {
    /// A worktree with an explicit name.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named(name.into())
    }
}
