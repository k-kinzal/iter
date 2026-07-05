//! Authentication commands: `hermes auth <command>`, `hermes login`, and
//! `hermes logout`.
//!
//! The Nous-portal OAuth flow shares a recurring option block across `login`,
//! `auth add`, and `model`; it is modeled once as [`NousOauthOptions`].

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_num, push_opt_path, push_positional};
use crate::values::{CredentialType, LoginProvider, LogoutProvider, SpotifyAction};

/// The Nous-portal OAuth option block shared by `login`, `auth add`, and
/// `model`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NousOauthOptions {
    /// `--portal-url <URL>`: portal base URL for Nous login.
    pub portal_url: Option<String>,
    /// `--inference-url <URL>`: inference API base URL.
    pub inference_url: Option<String>,
    /// `--client-id <ID>`: OAuth client id.
    pub client_id: Option<String>,
    /// `--scope <SCOPE>`: OAuth scope override.
    pub scope: Option<String>,
    /// `--no-browser`: do not auto-open a browser during login.
    pub no_browser: bool,
    /// `--timeout <SECS>`: HTTP request timeout in seconds.
    pub timeout: Option<u32>,
    /// `--ca-bundle <PATH>`: CA bundle PEM file for TLS verification.
    pub ca_bundle: Option<PathBuf>,
    /// `--insecure`: disable TLS verification (testing only).
    pub insecure: bool,
}

impl NousOauthOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--portal-url", self.portal_url.as_deref());
        push_opt(args, "--inference-url", self.inference_url.as_deref());
        push_opt(args, "--client-id", self.client_id.as_deref());
        push_opt(args, "--scope", self.scope.as_deref());
        push_flag(args, self.no_browser, "--no-browser");
        push_opt_num(args, "--timeout", self.timeout);
        push_opt_path(args, "--ca-bundle", self.ca_bundle.as_deref());
        push_flag(args, self.insecure, "--insecure");
    }
}

/// Options for `hermes auth add <PROVIDER>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthAddOptions {
    /// The provider id (e.g. `anthropic`, `openai-codex`, `openrouter`).
    pub provider: String,
    /// `--type <TYPE>`: credential type to add.
    pub credential_type: Option<CredentialType>,
    /// `--label <LABEL>`: optional display label.
    pub label: Option<String>,
    /// `--api-key <KEY>`: API key value (otherwise prompted).
    pub api_key: Option<String>,
    /// `--manual-paste`: skip the loopback callback listener.
    pub manual_paste: bool,
    /// The shared Nous OAuth option block.
    pub oauth: NousOauthOptions,
}

impl AuthAddOptions {
    /// Options for `hermes auth add <PROVIDER>` with only the provider set.
    #[must_use]
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            ..Self::default()
        }
    }
}

/// A `hermes auth` leaf subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthSubcommand {
    /// `add <PROVIDER>`: add a pooled credential.
    Add(AuthAddOptions),
    /// `list [PROVIDER]`: list pooled credentials.
    List {
        /// Optional provider filter.
        provider: Option<String>,
    },
    /// `remove <PROVIDER> <TARGET>`: remove a credential by index, id, or label.
    Remove {
        /// The provider id.
        provider: String,
        /// Credential index, entry id, or exact label.
        target: String,
    },
    /// `reset <PROVIDER>`: clear exhaustion status for a provider.
    Reset {
        /// The provider id.
        provider: String,
    },
    /// `status <PROVIDER>`: show auth status for a provider.
    Status {
        /// The provider id.
        provider: String,
    },
    /// `logout <PROVIDER>`: log out a provider and clear stored auth state.
    Logout {
        /// The provider id.
        provider: String,
    },
    /// `spotify [ACTION]`: authenticate Hermes with Spotify via PKCE.
    Spotify {
        /// The optional action (`login`, `status`, or `logout`).
        action: Option<SpotifyAction>,
    },
}

impl AuthSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add(options) => {
                args.push("add".into());
                if let Some(credential_type) = options.credential_type {
                    push_opt(args, "--type", Some(credential_type.as_str()));
                }
                push_opt(args, "--label", options.label.as_deref());
                push_opt(args, "--api-key", options.api_key.as_deref());
                push_flag(args, options.manual_paste, "--manual-paste");
                options.oauth.render(args);
                push_positional(args, &options.provider);
            }
            Self::List { provider } => {
                args.push("list".into());
                if let Some(provider) = provider {
                    push_positional(args, provider);
                }
            }
            Self::Remove { provider, target } => {
                args.push("remove".into());
                push_positional(args, provider);
                push_positional(args, target);
            }
            Self::Reset { provider } => {
                args.push("reset".into());
                push_positional(args, provider);
            }
            Self::Status { provider } => {
                args.push("status".into());
                push_positional(args, provider);
            }
            Self::Logout { provider } => {
                args.push("logout".into());
                push_positional(args, provider);
            }
            Self::Spotify { action } => {
                args.push("spotify".into());
                if let Some(action) = action {
                    push_positional(args, action.as_str());
                }
            }
        }
    }
}

/// `hermes auth <command>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    /// The leaf subcommand.
    pub subcommand: AuthSubcommand,
}

impl AuthCommand {
    /// Wrap an [`AuthSubcommand`].
    #[must_use]
    pub fn new(subcommand: AuthSubcommand) -> Self {
        Self { subcommand }
    }

    /// `hermes auth list`.
    #[must_use]
    pub fn list() -> Self {
        Self::new(AuthSubcommand::List { provider: None })
    }

    /// `hermes auth status <PROVIDER>`.
    #[must_use]
    pub fn status(provider: impl Into<String>) -> Self {
        Self::new(AuthSubcommand::Status {
            provider: provider.into(),
        })
    }
}

impl ToArgs for AuthCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("auth".into());
        self.subcommand.render(args);
    }
}

/// `hermes login` — run the OAuth device authorization flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginCommand {
    /// `--provider <PROVIDER>`: provider to authenticate with (default `nous`).
    pub provider: Option<LoginProvider>,
    /// The shared Nous OAuth option block.
    pub oauth: NousOauthOptions,
}

impl LoginCommand {
    /// `hermes login --provider <PROVIDER>`.
    #[must_use]
    pub fn provider(provider: LoginProvider) -> Self {
        Self {
            provider: Some(provider),
            ..Self::default()
        }
    }
}

impl ToArgs for LoginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("login".into());
        if let Some(provider) = self.provider {
            push_opt(args, "--provider", Some(provider.as_str()));
        }
        self.oauth.render(args);
    }
}

/// `hermes logout` — remove stored credentials and reset provider config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoutCommand {
    /// `--provider <PROVIDER>`: provider to log out from (default: active
    /// provider).
    pub provider: Option<LogoutProvider>,
}

impl LogoutCommand {
    /// `hermes logout --provider <PROVIDER>`.
    #[must_use]
    pub fn provider(provider: LogoutProvider) -> Self {
        Self {
            provider: Some(provider),
        }
    }
}

impl ToArgs for LogoutCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("logout".into());
        if let Some(provider) = self.provider {
            push_opt(args, "--provider", Some(provider.as_str()));
        }
    }
}
