use std::ffi::OsString;

use crate::args::{push_flag, push_opt};
use crate::values::Switch;

/// `claude auth ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Auth {
    /// `auth login`.
    Login(AuthLogin),
    /// `auth logout`.
    Logout,
    /// `auth status`.
    Status(AuthStatus),
    /// `auth help [command]`.
    Help(Option<AuthHelpCommand>),
}

impl Auth {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Login(command) => {
                args.push("login".into());
                command.render(args);
            }
            Self::Logout => args.push("logout".into()),
            Self::Status(command) => {
                args.push("status".into());
                command.render(args);
            }
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
        }
    }
}

/// `claude auth help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthHelpCommand {
    /// `login`.
    Login,
    /// `logout`.
    Logout,
    /// `status`.
    Status,
}

impl AuthHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Logout => "logout",
            Self::Status => "status",
        }
    }
}

/// `claude auth login`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthLogin {
    /// Login provider selection.
    pub provider: Option<AuthLoginProvider>,
    /// `--email`.
    pub email: Option<String>,
    /// `--sso`.
    pub sso: Switch,
}

impl AuthLogin {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        if let Some(provider) = self.provider {
            args.push(provider.as_flag().into());
        }
        push_opt(args, "--email", self.email.as_deref());
        push_flag(args, self.sso, "--sso");
    }
}

/// Mutually exclusive `claude auth login` provider flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthLoginProvider {
    /// `--claudeai`.
    ClaudeAi,
    /// `--console`.
    Console,
}

impl AuthLoginProvider {
    #[must_use]
    const fn as_flag(self) -> &'static str {
        match self {
            Self::ClaudeAi => "--claudeai",
            Self::Console => "--console",
        }
    }
}

/// `claude auth status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthStatus {
    /// Output format selection.
    pub format: Option<AuthStatusFormat>,
}

impl AuthStatus {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        if let Some(format) = self.format {
            args.push(format.as_flag().into());
        }
    }
}

/// Mutually exclusive `claude auth status` output flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthStatusFormat {
    /// `--json`.
    Json,
    /// `--text`.
    Text,
}

impl AuthStatusFormat {
    #[must_use]
    const fn as_flag(self) -> &'static str {
        match self {
            Self::Json => "--json",
            Self::Text => "--text",
        }
    }
}
