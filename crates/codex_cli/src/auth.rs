//! Authentication subcommands: `login` (+ `login status`) and `logout`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};
use crate::options::GlobalConfig;

/// `codex login [OPTIONS] [COMMAND]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--with-api-key`: read the API key from stdin.
    pub with_api_key: bool,
    /// `--with-access-token`: read the access token from stdin.
    pub with_access_token: bool,
    /// `--device-auth`: use the device-authorization flow.
    pub device_auth: bool,
    /// Optional `status` subcommand.
    pub status: bool,
}

impl LoginCommand {
    /// Build `codex login status`.
    #[must_use]
    pub fn status() -> Self {
        Self {
            status: true,
            ..Self::default()
        }
    }
}

impl ToArgs for LoginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("login".into());
        self.global.render(args);
        push_flag(args, self.with_api_key, "--with-api-key");
        push_flag(args, self.with_access_token, "--with-access-token");
        push_flag(args, self.device_auth, "--device-auth");
        if self.status {
            args.push("status".into());
        }
    }
}

/// `codex logout [OPTIONS]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogoutCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
}

impl ToArgs for LogoutCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("logout".into());
        self.global.render(args);
    }
}
