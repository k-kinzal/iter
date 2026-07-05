//! `codex [OPTIONS] [PROMPT]` — the root interactive run.
//!
//! With no subcommand, Codex launches its TUI seeded by an optional prompt
//! positional. The root command adds a few options on top of the shared
//! [`CommonConfig`] (approval policy, web search, alt-screen control, and the
//! remote-execution flags).

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_enum, push_opt};
use crate::options::CommonConfig;
use crate::values::ApprovalPolicy;

/// Options specific to the root `codex` run (beyond [`CommonConfig`]).
///
/// These are also shared by the interactive `resume` and `fork` subcommands,
/// whose `--help` lists the same connection and approval flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `--remote <ADDR>`: connect the TUI to a remote app server endpoint
    /// (`ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`).
    pub remote: Option<String>,
    /// `--remote-auth-token-env <ENV_VAR>`: name of the environment variable
    /// holding the bearer token sent to a remote app server websocket.
    pub remote_auth_token_env: Option<String>,
    /// `-a, --ask-for-approval <APPROVAL_POLICY>`.
    pub ask_for_approval: Option<ApprovalPolicy>,
    /// `--search`: enable the web-search tool.
    pub search: bool,
    /// `--no-alt-screen`: keep the TUI in the main terminal screen.
    pub no_alt_screen: bool,
}

impl RunOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--remote", self.remote.as_deref());
        push_opt(
            args,
            "--remote-auth-token-env",
            self.remote_auth_token_env.as_deref(),
        );
        push_enum(
            args,
            "--ask-for-approval",
            self.ask_for_approval.map(ApprovalPolicy::as_str),
        );
        push_flag(args, self.search, "--search");
        push_flag(args, self.no_alt_screen, "--no-alt-screen");
    }
}

/// `codex [OPTIONS] [PROMPT]` — the root interactive run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// Options common to the root run and `exec`.
    pub common: CommonConfig,
    /// Root-run-specific options.
    pub options: RunOptions,
    /// Optional prompt positional seeding the first turn.
    pub prompt: Option<String>,
}

impl RunCommand {
    /// Build a root run seeded with a prompt positional.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.common.render(args);
        self.options.render(args);
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}
