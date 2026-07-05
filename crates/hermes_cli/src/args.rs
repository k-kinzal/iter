//! The [`ToArgs`] trait and the small argv push-helpers shared by every
//! command builder.

use std::ffi::{OsStr, OsString};
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

/// Push a bare `flag` when `enabled`.
pub(crate) fn push_flag(args: &mut Vec<OsString>, enabled: bool, flag: &'static str) {
    if enabled {
        args.push(flag.into());
    }
}

/// Push `flag` followed by its `value`.
pub(crate) fn push_pair(args: &mut Vec<OsString>, flag: &'static str, value: impl AsRef<OsStr>) {
    args.push(flag.into());
    args.push(value.as_ref().into());
}

/// Push `flag value` when `value` is present.
pub(crate) fn push_opt(args: &mut Vec<OsString>, flag: &'static str, value: Option<&str>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

/// Push `flag <path>` when `value` is present.
pub(crate) fn push_opt_path(args: &mut Vec<OsString>, flag: &'static str, value: Option<&Path>) {
    if let Some(value) = value {
        push_pair(args, flag, value);
    }
}

/// Push `flag <n>` when `value` is present, rendering the number as a decimal.
pub(crate) fn push_opt_num(args: &mut Vec<OsString>, flag: &'static str, value: Option<u32>) {
    if let Some(value) = value {
        push_pair(args, flag, value.to_string());
    }
}

/// Push a required positional operand.
pub(crate) fn push_positional(args: &mut Vec<OsString>, value: impl AsRef<OsStr>) {
    args.push(value.as_ref().into());
}

/// Push an optional positional operand when present.
pub(crate) fn push_opt_positional(args: &mut Vec<OsString>, value: Option<&str>) {
    if let Some(value) = value {
        args.push(value.into());
    }
}

/// Push each string in `values` as a bare positional operand.
pub(crate) fn push_positionals(args: &mut Vec<OsString>, values: &[String]) {
    for value in values {
        args.push(value.into());
    }
}
