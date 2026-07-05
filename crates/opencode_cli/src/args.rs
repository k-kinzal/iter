//! The [`ToArgs`] trait and the small argv-rendering helpers the command
//! builders share.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

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

pub(crate) fn push_enum(args: &mut Vec<OsString>, flag: &'static str, value: Option<&'static str>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

/// Render an optional value-taking flag whose value is any [`Display`] type
/// (used for opencode's numeric flags: `--port`, `--days`, `--tools`).
///
/// [`Display`]: std::fmt::Display
pub(crate) fn push_opt_display<T: std::fmt::Display>(
    args: &mut Vec<OsString>,
    flag: &'static str,
    value: Option<T>,
) {
    if let Some(value) = value {
        push_pair(args, flag, value.to_string());
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
