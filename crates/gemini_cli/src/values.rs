//! Typed value enums for Gemini's value-taking flags.

/// `-o, --output-format <FORMAT>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `text` — human-readable output (the default).
    Text,
    /// `json` — a single terminal JSON record.
    Json,
    /// `stream-json` — newline-delimited JSON events.
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

/// `--approval-mode <MODE>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalMode {
    /// `default` — prompt for approval.
    Default,
    /// `auto_edit` — auto-approve edit tools.
    AutoEdit,
    /// `yolo` — auto-approve all tools.
    Yolo,
    /// `plan` — read-only mode.
    Plan,
}

impl ApprovalMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AutoEdit => "auto_edit",
            Self::Yolo => "yolo",
            Self::Plan => "plan",
        }
    }
}

/// `-s, --scope <SCOPE>` for `gemini mcp add` / `gemini mcp remove`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpScope {
    /// `user` — the user-level configuration.
    User,
    /// `project` — the project-level configuration (the default).
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

/// `-t, --transport <TYPE>` for `gemini mcp add`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// `stdio` — a local stdio server (the default).
    Stdio,
    /// `sse` — a Server-Sent Events server.
    Sse,
    /// `http` — a streamable HTTP server.
    Http,
}

impl McpTransport {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
        }
    }
}

/// `--scope <SCOPE>` for `gemini extensions config` and the fixed-choice
/// `gemini skills` leaves (`disable` / `install` / `link`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    /// `user` — the user-level (global) scope.
    User,
    /// `workspace` — the workspace-level scope.
    Workspace,
}

impl Scope {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

/// `-o, --output-format <FORMAT>` for `gemini extensions list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionsOutputFormat {
    /// `text` — human-readable output (the default).
    Text,
    /// `json` — machine-readable JSON.
    Json,
}

impl ExtensionsOutputFormat {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// The `template` positional for `gemini extensions new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionTemplate {
    /// `custom-commands`.
    CustomCommands,
    /// `exclude-tools`.
    ExcludeTools,
    /// `hooks`.
    Hooks,
    /// `mcp-server`.
    McpServer,
    /// `policies`.
    Policies,
    /// `skills`.
    Skills,
    /// `themes-example`.
    ThemesExample,
}

impl ExtensionTemplate {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CustomCommands => "custom-commands",
            Self::ExcludeTools => "exclude-tools",
            Self::Hooks => "hooks",
            Self::McpServer => "mcp-server",
            Self::Policies => "policies",
            Self::Skills => "skills",
            Self::ThemesExample => "themes-example",
        }
    }
}

/// `-r, --resume <SESSION>` — the session selector for the root run.
///
/// The flag accepts only `latest` (the most recent session) or an index number
/// (e.g. `--resume 5`); no other string resolves to a session. Modeling it as a
/// closed union keeps out-of-range values like `banana` unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionRef {
    /// `latest` — resume the most recent session.
    Latest,
    /// `<INDEX>` — resume the session at this index number.
    Index(u64),
}

impl SessionRef {
    /// Render the `--resume` value: `latest` or the index number.
    #[must_use]
    pub(crate) fn as_arg(&self) -> String {
        match self {
            Self::Latest => "latest".to_owned(),
            Self::Index(index) => index.to_string(),
        }
    }
}

/// `-w, --worktree [NAME]` — start Gemini in a new git worktree.
///
/// The flag takes an optional value: passed bare it generates a worktree name
/// automatically; passed a value it uses that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Worktree {
    /// `--worktree` with no value — Gemini generates a name.
    Auto,
    /// `--worktree <NAME>` — use the given worktree name.
    Named(String),
}

impl Worktree {
    /// Build a named worktree selection.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named(name.into())
    }
}
