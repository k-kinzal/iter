//! Operational subcommands: `completion`, `login`, `update`, `version`,
//! `init`, and `help`.
//!
//! These are the standalone management leaves off the `copilot` root. None
//! takes options beyond what is modeled here (each was verified against its
//! own `--help`).

use std::ffi::OsString;

use crate::args::{ToArgs, push_opt};
use crate::values::{Shell, UpdateChannel};

/// `copilot completion <SHELL>` — write a shell completion script to stdout.
///
/// The shell is a **required** positional (`bash`/`zsh`/`fish`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionCommand {
    /// Target shell.
    pub shell: Shell,
}

impl CompletionCommand {
    /// Build a completion command for `shell`.
    #[must_use]
    pub const fn new(shell: Shell) -> Self {
        Self { shell }
    }
}

impl ToArgs for CompletionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("completion".into());
        args.push(self.shell.as_str().into());
    }
}

/// `copilot login [--host <host>]` — authenticate via OAuth device flow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginCommand {
    /// `--host <host>`: a GitHub Enterprise Cloud host (defaults to
    /// `https://github.com`).
    pub host: Option<String>,
}

impl ToArgs for LoginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("login".into());
        push_opt(args, "--host", self.host.as_deref());
    }
}

/// `copilot update [CHANNEL]` — download the latest version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand {
    /// Update channel; omit for the default stable channel.
    pub channel: Option<UpdateChannel>,
}

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
        if let Some(channel) = self.channel {
            args.push(channel.as_str().into());
        }
    }
}

/// `copilot version` — display version information and check for updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VersionCommand;

impl ToArgs for VersionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("version".into());
    }
}

/// `copilot init` — initialize `.github/copilot-instructions.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InitCommand;

impl ToArgs for InitCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("init".into());
    }
}

/// `copilot help [TOPIC]` — display help, optionally for a named topic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelpCommand {
    /// Help topic (e.g. `environment`, `permissions`); omit for the main page.
    pub topic: Option<String>,
}

impl ToArgs for HelpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("help".into());
        if let Some(topic) = &self.topic {
            args.push(topic.into());
        }
    }
}
