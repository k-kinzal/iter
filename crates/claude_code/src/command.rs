use std::ffi::OsString;

use crate::agents::Agents;
use crate::args::ToArgs;
use crate::auth::Auth;
use crate::auto_mode::AutoMode;
use crate::install::Install;
use crate::mcp::Mcp;
use crate::plugin::Plugin;
use crate::project::Project;
use crate::ultrareview::UltraReview;

/// Top-level Claude Code command surface.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub(crate) enum ClaudeCodeCommand {
    /// `claude agents`.
    Agents(Agents),
    /// `claude auth ...`.
    Auth(Auth),
    /// `claude auto-mode ...`.
    AutoMode(AutoMode),
    /// `claude doctor`.
    Doctor,
    /// `claude install`.
    Install(Install),
    /// `claude mcp ...`.
    Mcp(Mcp),
    /// `claude plugin ...`.
    Plugin(Plugin),
    /// `claude project ...`.
    Project(Project),
    /// `claude setup-token`.
    SetupToken,
    /// `claude ultrareview`.
    UltraReview(UltraReview),
    /// `claude update`.
    Update,
}

/// `claude doctor`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Doctor;

/// `claude setup-token`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetupToken;

/// `claude update`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Update;

impl ToArgs for ClaudeCodeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Agents(command) => {
                args.push("agents".into());
                command.render(args);
            }
            Self::Auth(command) => {
                args.push("auth".into());
                command.render(args);
            }
            Self::AutoMode(command) => {
                args.push("auto-mode".into());
                command.render(args);
            }
            Self::Doctor => args.push("doctor".into()),
            Self::Install(command) => {
                args.push("install".into());
                command.render(args);
            }
            Self::Mcp(command) => {
                args.push("mcp".into());
                command.render(args);
            }
            Self::Plugin(command) => {
                args.push("plugin".into());
                command.render(args);
            }
            Self::Project(command) => {
                args.push("project".into());
                command.render(args);
            }
            Self::SetupToken => args.push("setup-token".into()),
            Self::UltraReview(command) => {
                args.push("ultrareview".into());
                command.render(args);
            }
            Self::Update => args.push("update".into()),
        }
    }
}

impl From<Agents> for ClaudeCodeCommand {
    fn from(value: Agents) -> Self {
        Self::Agents(value)
    }
}

impl From<Auth> for ClaudeCodeCommand {
    fn from(value: Auth) -> Self {
        Self::Auth(value)
    }
}

impl From<AutoMode> for ClaudeCodeCommand {
    fn from(value: AutoMode) -> Self {
        Self::AutoMode(value)
    }
}

impl From<Install> for ClaudeCodeCommand {
    fn from(value: Install) -> Self {
        Self::Install(value)
    }
}

impl From<Mcp> for ClaudeCodeCommand {
    fn from(value: Mcp) -> Self {
        Self::Mcp(value)
    }
}

impl From<Plugin> for ClaudeCodeCommand {
    fn from(value: Plugin) -> Self {
        Self::Plugin(value)
    }
}

impl From<Project> for ClaudeCodeCommand {
    fn from(value: Project) -> Self {
        Self::Project(value)
    }
}

impl From<UltraReview> for ClaudeCodeCommand {
    fn from(value: UltraReview) -> Self {
        Self::UltraReview(value)
    }
}

impl From<Doctor> for ClaudeCodeCommand {
    fn from(_: Doctor) -> Self {
        Self::Doctor
    }
}

impl From<SetupToken> for ClaudeCodeCommand {
    fn from(_: SetupToken) -> Self {
        Self::SetupToken
    }
}

impl From<Update> for ClaudeCodeCommand {
    fn from(_: Update) -> Self {
        Self::Update
    }
}

impl ToArgs for Agents {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Auth {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for AutoMode {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Install {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Mcp {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Plugin {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Project {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for UltraReview {
    fn write_args(&self, args: &mut Vec<OsString>) {
        ClaudeCodeCommand::from(self.clone()).write_args(args);
    }
}

impl ToArgs for Doctor {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("doctor".into());
    }
}

impl ToArgs for SetupToken {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("setup-token".into());
    }
}

impl ToArgs for Update {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
    }
}
