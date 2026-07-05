//! `opencode auth` — manage provider credentials.

use std::ffi::OsString;

use crate::args::{ToArgs, push_opt};
use crate::options::GlobalOptions;

/// `opencode auth <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The auth subcommand.
    pub command: AuthSubcommand,
}

impl AuthCommand {
    /// Wrap an auth subcommand with default global options.
    #[must_use]
    pub fn new(command: AuthSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for AuthCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("auth".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode auth` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthSubcommand {
    /// `auth login [url]`: log in to a provider.
    Login {
        /// Optional provider URL positional.
        url: Option<String>,
        /// `-p, --provider <PROVIDER>`: provider id or name to log in to
        /// (skips provider selection).
        provider: Option<String>,
        /// `-m, --method <METHOD>`: login method label (skips method selection).
        method: Option<String>,
    },
    /// `auth logout`: log out from a configured provider.
    Logout,
    /// `auth list` (alias `ls`): list providers.
    List,
}

impl AuthSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Login {
                url,
                provider,
                method,
            } => {
                args.push("login".into());
                push_opt(args, "--provider", provider.as_deref());
                push_opt(args, "--method", method.as_deref());
                if let Some(url) = url {
                    args.push(url.into());
                }
            }
            Self::Logout => args.push("logout".into()),
            Self::List => args.push("list".into()),
        }
    }
}
