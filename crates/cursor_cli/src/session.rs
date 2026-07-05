//! Chat/session subcommands: `create-chat`, `ls`, and `resume`.
//!
//! These take no options beyond `--help` in the pinned CLI, so they are modeled
//! as bare command tokens. Session continuity for a *run* is expressed through
//! the root run's `--resume` / `--continue` options (see
//! [`RunOptions`](crate::RunOptions)).

use std::ffi::OsString;

use crate::args::ToArgs;

/// `cursor-agent create-chat` — create a new empty chat and return its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateChatCommand;

impl ToArgs for CreateChatCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("create-chat".into());
    }
}

/// `cursor-agent ls` — pick a chat session to resume.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsCommand;

impl ToArgs for LsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("ls".into());
    }
}

/// `cursor-agent resume` — resume the latest chat session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeCommand;

impl ToArgs for ResumeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("resume".into());
    }
}
