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

/// Render a default-true boolean: `on_flag` when `true`, `off_flag` (the
/// yargs `--no-<flag>` negation) when `false`. Unlike [`push_flag`], the
/// off-state is always emitted, so `false` reliably expresses "off" instead of
/// silently falling back to the CLI's `default: true`.
pub(crate) fn push_bool(
    args: &mut Vec<OsString>,
    enabled: bool,
    on_flag: &'static str,
    off_flag: &'static str,
) {
    let flag = if enabled { on_flag } else { off_flag };
    args.push(flag.into());
}

/// Render repeatable `KEY<sep>VALUE` pairs (`-e KEY=value`, `-H "Name: value"`)
/// as one `flag` + joined `key{separator}value` argv entry per pair.
pub(crate) fn push_pairs(
    args: &mut Vec<OsString>,
    flag: &'static str,
    separator: &str,
    pairs: &[(String, String)],
) {
    for (key, value) in pairs {
        push_pair(args, flag, format!("{key}{separator}{value}"));
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

pub(crate) fn push_num(args: &mut Vec<OsString>, flag: &'static str, value: Option<impl ToString>) {
    if let Some(value) = value {
        push_pair(args, flag, value.to_string());
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
