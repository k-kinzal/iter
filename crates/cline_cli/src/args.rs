use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::path::Path;

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

/// Render a value-taking flag whose value is a number (or any [`Display`]).
pub(crate) fn push_opt_num<T: Display>(
    args: &mut Vec<OsString>,
    flag: &'static str,
    value: Option<T>,
) {
    if let Some(value) = value {
        push_pair(args, flag, value.to_string());
    }
}

/// Render Cline's `--auto-approve <boolean>` shape: the flag takes a literal
/// `true`/`false` value rather than being a bare switch.
pub(crate) fn push_bool(args: &mut Vec<OsString>, flag: &'static str, value: Option<bool>) {
    if let Some(value) = value {
        push_pair(args, flag, if value { "true" } else { "false" });
    }
}
