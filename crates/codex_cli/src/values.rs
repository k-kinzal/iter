//! Typed value enums for Codex's value-taking flags.

/// `-s, --sandbox <SANDBOX_MODE>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SandboxMode {
    /// `read-only`.
    ReadOnly,
    /// `workspace-write`.
    WorkspaceWrite,
    /// `danger-full-access`.
    DangerFullAccess,
}

impl SandboxMode {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

/// `-a, --ask-for-approval <APPROVAL_POLICY>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalPolicy {
    /// `untrusted`.
    Untrusted,
    /// `on-failure` (deprecated by Codex, still accepted).
    OnFailure,
    /// `on-request`.
    OnRequest,
    /// `never`.
    Never,
}

impl ApprovalPolicy {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

/// `--color <COLOR>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Color {
    /// `always`.
    Always,
    /// `never`.
    Never,
    /// `auto`.
    Auto,
}

impl Color {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::Auto => "auto",
        }
    }
}

/// The shell a `codex completion` script targets.
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

/// `--local-provider <OSS_PROVIDER>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalProvider {
    /// `lmstudio`.
    LmStudio,
    /// `ollama`.
    Ollama,
}

impl LocalProvider {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LmStudio => "lmstudio",
            Self::Ollama => "ollama",
        }
    }
}

/// A single `-c/--config key=value` override.
///
/// Codex parses the value portion as TOML, falling back to a literal string.
/// The crate treats it as opaque text and renders `key=value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOverride {
    /// Dotted config path, e.g. `model` or `shell_environment_policy.inherit`.
    pub key: String,
    /// Raw value text (TOML or literal).
    pub value: String,
}

impl ConfigOverride {
    /// Build a config override.
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    #[must_use]
    pub(crate) fn render(&self) -> String {
        format!("{}={}", self.key, self.value)
    }
}
