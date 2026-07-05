//! The server-hosting commands: `acp`, `serve`, and `web`.
//!
//! All three stand up an opencode server and share the [`ServerOptions`] bind
//! and discovery flags; `acp` adds a `--cwd` working-directory override.

use std::ffi::OsString;

use crate::args::{ToArgs, push_opt};
use crate::options::{GlobalOptions, ServerOptions};

/// `opencode acp [OPTIONS]` — start an ACP (Agent Client Protocol) server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// Server bind and discovery options.
    pub server: ServerOptions,
    /// `--cwd <DIR>`: working directory.
    pub cwd: Option<String>,
}

impl ToArgs for AcpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("acp".into());
        self.global.render(args);
        self.server.render(args);
        push_opt(args, "--cwd", self.cwd.as_deref());
    }
}

/// `opencode serve [OPTIONS]` — start a headless opencode server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// Server bind and discovery options.
    pub server: ServerOptions,
}

impl ToArgs for ServeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("serve".into());
        self.global.render(args);
        self.server.render(args);
    }
}

/// `opencode web [OPTIONS]` — start a server and open the web interface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// Server bind and discovery options.
    pub server: ServerOptions,
}

impl ToArgs for WebCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("web".into());
        self.global.render(args);
        self.server.render(args);
    }
}
