//! Authentication subcommands: `login`, `logout`, and `status` (alias
//! `whoami`).
//!
//! Each takes no options beyond `--help` in the pinned CLI, so they are modeled
//! as bare command tokens.

use std::ffi::OsString;

use crate::args::ToArgs;

/// `cursor-agent login` — authenticate with Cursor.
///
/// Honors `NO_OPEN_BROWSER` in the environment to disable browser opening.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginCommand;

impl ToArgs for LoginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("login".into());
    }
}

/// `cursor-agent logout` — sign out and clear stored authentication.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoutCommand;

impl ToArgs for LogoutCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("logout".into());
    }
}

/// `cursor-agent status` (alias `whoami`) — view authentication status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusCommand;

impl ToArgs for StatusCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("status".into());
    }
}
