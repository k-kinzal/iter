use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::values::{OptionalValue, SettingSource};

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

pub(crate) fn push_flag(args: &mut Vec<OsString>, enabled: impl Into<bool>, flag: &'static str) {
    if enabled.into() {
        args.push(flag.into());
    }
}

pub(crate) fn push_pair(args: &mut Vec<OsString>, flag: &'static str, value: impl AsRef<OsStr>) {
    args.push(flag.into());
    args.push(value.as_ref().into());
}

pub(crate) fn push_pair_os(args: &mut Vec<OsString>, flag: &'static str, value: OsString) {
    args.push(flag.into());
    args.push(value);
}

pub(crate) fn push_positional_boundary(args: &mut Vec<OsString>) {
    if args.last().is_none_or(|arg| arg != "--") {
        args.push("--".into());
    }
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

pub(crate) fn push_joined(args: &mut Vec<OsString>, flag: &'static str, values: &[String]) {
    if !values.is_empty() {
        push_pair(args, flag, values.join(","));
    }
}

pub(crate) fn push_setting_sources(args: &mut Vec<OsString>, values: &[SettingSource]) {
    if !values.is_empty() {
        let joined = values
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join(",");
        push_pair(args, "--setting-sources", joined);
    }
}

pub(crate) fn push_optional_value<T, F>(
    args: &mut Vec<OsString>,
    flag: &'static str,
    value: Option<&OptionalValue<T>>,
    render: F,
) where
    F: Fn(&T) -> String,
{
    match value {
        Some(OptionalValue::Present) => args.push(flag.into()),
        Some(OptionalValue::Value(value)) => {
            let mut arg = String::from(flag);
            arg.push('=');
            arg.push_str(&render(value));
            args.push(arg.into());
        }
        None => {}
    }
}
