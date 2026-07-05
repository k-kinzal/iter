//! Typed value types for Hermes' choice-constrained flags and positionals.
//!
//! Each mirrors an argparse `choices=[...]` set, so an invalid value is
//! unrepresentable rather than caught at run time by the CLI.

/// Provider for `hermes login --provider` and `hermes auth add`'s OAuth flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginProvider {
    /// `nous` — Nous Portal (the default).
    Nous,
    /// `openai-codex`.
    OpenAiCodex,
    /// `xai-oauth`.
    XaiOauth,
}

impl LoginProvider {
    /// The provider's CLI token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nous => "nous",
            Self::OpenAiCodex => "openai-codex",
            Self::XaiOauth => "xai-oauth",
        }
    }
}

/// Provider for `hermes logout --provider`. Adds `spotify` to the
/// [`LoginProvider`] set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogoutProvider {
    /// `nous`.
    Nous,
    /// `openai-codex`.
    OpenAiCodex,
    /// `xai-oauth`.
    XaiOauth,
    /// `spotify`.
    Spotify,
}

impl LogoutProvider {
    /// The provider's CLI token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nous => "nous",
            Self::OpenAiCodex => "openai-codex",
            Self::XaiOauth => "xai-oauth",
            Self::Spotify => "spotify",
        }
    }
}

/// Credential type for `hermes auth add --type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialType {
    /// `oauth`.
    Oauth,
    /// `api-key`.
    ApiKey,
}

impl CredentialType {
    /// The credential type's CLI token. `api-key` is rendered in its hyphenated
    /// spelling (the CLI also accepts `api_key`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::ApiKey => "api-key",
        }
    }
}

/// Auth method for `hermes mcp add --auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpAuth {
    /// `oauth`.
    Oauth,
    /// `header`.
    Header,
}

impl McpAuth {
    /// The auth method's CLI token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::Header => "header",
        }
    }
}

/// Action for `hermes auth spotify [ACTION]`. Mirrors the argparse
/// `{login,status,logout}` choice set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpotifyAction {
    /// `login`.
    Login,
    /// `status`.
    Status,
    /// `logout`.
    Logout,
}

impl SpotifyAction {
    /// The action's CLI token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Status => "status",
            Self::Logout => "logout",
        }
    }
}

/// Shell selector for `hermes completion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Shell {
    /// `bash` (the CLI default).
    #[default]
    Bash,
    /// `zsh`.
    Zsh,
    /// `fish`.
    Fish,
}

impl Shell {
    /// The shell's CLI token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }
}
