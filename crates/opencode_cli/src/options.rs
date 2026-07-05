//! Options shared across opencode's command tree.
//!
//! opencode is a yargs CLI: two option groups recur on almost every command.
//! [`GlobalOptions`] (`--print-logs`, `--log-level`) appears on every command;
//! [`ServerOptions`] (`--port`, `--hostname`, the mDNS pair, `--cors`) appears
//! on the server-hosting commands (the root TUI, `acp`, `serve`, `web`).

use std::ffi::OsString;

use crate::args::{push_each, push_enum, push_flag, push_opt, push_opt_display};
use crate::values::LogLevel;

/// The `--print-logs` / `--log-level` options every opencode command accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalOptions {
    /// `--print-logs`: also write logs to stderr.
    pub print_logs: bool,
    /// `--log-level <LEVEL>`.
    pub log_level: Option<LogLevel>,
}

impl GlobalOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.print_logs, "--print-logs");
        push_enum(args, "--log-level", self.log_level.map(LogLevel::as_str));
    }
}

/// The server-hosting options shared by the root TUI, `acp`, `serve`, and
/// `web`: bind address plus mDNS service discovery and CORS allow-listing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerOptions {
    /// `--port <PORT>` (opencode defaults to `0`, i.e. a random port).
    pub port: Option<u16>,
    /// `--hostname <HOST>` (opencode defaults to `127.0.0.1`).
    pub hostname: Option<String>,
    /// `--mdns`: enable mDNS service discovery.
    pub mdns: bool,
    /// `--mdns-domain <DOMAIN>` (opencode defaults to `opencode.local`).
    pub mdns_domain: Option<String>,
    /// `--cors <DOMAIN>` (repeatable): additional CORS-allowed domains.
    pub cors: Vec<String>,
}

impl ServerOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt_display(args, "--port", self.port);
        push_opt(args, "--hostname", self.hostname.as_deref());
        push_flag(args, self.mdns, "--mdns");
        push_opt(args, "--mdns-domain", self.mdns_domain.as_deref());
        push_each(args, "--cors", &self.cors);
    }
}
