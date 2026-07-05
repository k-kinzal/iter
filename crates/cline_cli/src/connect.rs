//! `cline connect [CHANNEL]` — connect Cline to an external channel.
//!
//! Each channel (`slack`, …) exposes its own channel-specific option surface
//! that Cline only prints under `connect <channel> --help`. This builder
//! therefore models the stable top-level shape — the optional channel
//! positional, the `--stop` switch, and a raw passthrough for the
//! channel-specific flags — rather than enumerating every bridge's options.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag};

/// `cline connect [OPTIONS] [CHANNEL] [CHANNEL_ARGS]...`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectCommand {
    /// Optional `[CHANNEL]` positional (e.g. `slack`).
    pub channel: Option<String>,
    /// `--stop`: kill all current channel connections.
    pub stop: bool,
    /// Channel-specific flags passed through verbatim (e.g. `--base-url`,
    /// `--bot-token`).
    pub channel_args: Vec<String>,
}

impl ToArgs for ConnectCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("connect".into());
        push_flag(args, self.stop, "--stop");
        if let Some(channel) = &self.channel {
            args.push(channel.into());
        }
        args.extend(self.channel_args.iter().map(OsString::from));
    }
}
