//! The [`ToArgs`] trait and the small argv-rendering helpers the command
//! builders share.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Something that can render argv entries after the executable name.
pub trait ToArgs {
    /// Append argv entries after the executable name.
    fn write_args(&self, args: &mut Vec<OsString>);

    /// Render this value into argv entries after the executable name.
    #[must_use]
    fn to_args(&self) -> Vec<OsString> {
        let mut args = Vec::new();
        self.write_args(&mut args);
        args
    }
}

pub(crate) fn push_flag(args: &mut Vec<OsString>, enabled: bool, flag: &'static str) {
    if enabled {
        args.push(flag.into());
    }
}

pub(crate) fn push_pair(args: &mut Vec<OsString>, flag: &'static str, value: impl AsRef<OsStr>) {
    args.push(flag.into());
    args.push(value.as_ref().into());
}

pub(crate) fn push_opt(args: &mut Vec<OsString>, flag: &'static str, value: Option<&str>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

pub(crate) fn push_opt_path(args: &mut Vec<OsString>, flag: &'static str, value: Option<&Path>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

pub(crate) fn push_enum(args: &mut Vec<OsString>, flag: &'static str, value: Option<&'static str>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

pub(crate) fn push_each(args: &mut Vec<OsString>, flag: &'static str, values: &[String]) {
    for value in values {
        push_pair(args, flag, value);
    }
}

pub(crate) fn push_paths(args: &mut Vec<OsString>, flag: &'static str, values: &[PathBuf]) {
    for value in values {
        push_pair(args, flag, value);
    }
}

/// Render an optional value flag in Copilot's attached `--flag=value` form.
///
/// Copilot declares its enable/disable toggles as `--flag[=value]` (commander
/// optional-value syntax), which only accepts the value glued on with `=`.
pub(crate) fn push_attached(args: &mut Vec<OsString>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(format!("{flag}={value}").into());
    }
}

/// Render a repeatable value flag in Copilot's attached `--flag=value` form.
///
/// Copilot's tool/url list flags are declared `--flag[=values...]`; each value
/// is emitted as its own `--flag=value` entry so a repeated flag round-trips.
pub(crate) fn push_each_attached(args: &mut Vec<OsString>, flag: &str, values: &[String]) {
    for value in values {
        args.push(format!("{flag}={value}").into());
    }
}
