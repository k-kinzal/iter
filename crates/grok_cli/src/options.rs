//! Options every Grok subcommand accepts.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{push_flag, push_opt_path};

/// The `--debug` / `--debug-file` / `--leader-socket` options every Grok
/// command and subcommand accepts.
///
/// Grok threads these through even to management subcommands (`mcp`, `plugin`,
/// `sessions`, `worktree`, …), so they share this small struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalOptions {
    /// `--debug`: enable debug logging.
    pub debug: bool,
    /// `--debug-file <FILE>`: write debug logs to a file.
    pub debug_file: Option<PathBuf>,
    /// `--leader-socket <PATH>`: use a custom leader socket path.
    pub leader_socket: Option<PathBuf>,
}

impl GlobalOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.debug, "--debug");
        push_opt_path(args, "--debug-file", self.debug_file.as_deref());
        push_opt_path(args, "--leader-socket", self.leader_socket.as_deref());
    }
}
