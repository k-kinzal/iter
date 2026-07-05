//! `grok login` / `grok logout` — credential management.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};
use crate::options::GlobalOptions;

/// `grok login [OPTIONS]` — sign in to Grok.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// `--oauth`: use Grok OAuth via `auth.x.ai`.
    pub oauth: bool,
    /// `--device-auth`: use device-code authentication for headless/remote
    /// environments (alias `--device-code`).
    pub device_auth: bool,
}

impl ToArgs for LoginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("login".into());
        push_flag(args, self.oauth, "--oauth");
        push_flag(args, self.device_auth, "--device-auth");
        self.global.render(args);
    }
}

/// `grok logout [OPTIONS]` — sign out and clear cached credentials.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoutCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
}

impl ToArgs for LogoutCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("logout".into());
        self.global.render(args);
    }
}
