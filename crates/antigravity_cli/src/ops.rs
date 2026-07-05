//! The remaining `agy` subcommands: `install`, `models`, `update`,
//! `changelog`, and `help`.
//!
//! Each is text-only. Only `install` carries flags; the rest are bare
//! invocations (`-h` / `--help` is universal and not modeled per command).

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt_path};

/// `agy install [flags]` — configure environment paths and shell settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallCommand {
    /// `--dir <DIR>`: custom directory target to configure PATH for.
    pub dir: Option<PathBuf>,
    /// `--skip-aliases`: bypass shell-profile alias purging.
    pub skip_aliases: bool,
    /// `--skip-path`: bypass shell-profile PATH appending.
    pub skip_path: bool,
}

impl ToArgs for InstallCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("install".into());
        push_opt_path(args, "--dir", self.dir.as_deref());
        push_flag(args, self.skip_aliases, "--skip-aliases");
        push_flag(args, self.skip_path, "--skip-path");
    }
}

/// `agy models` — list available models.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsCommand;

impl ToArgs for ModelsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("models".into());
    }
}

/// `agy update` — update the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand;

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
    }
}

/// `agy changelog` — show the changelog and release notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangelogCommand;

impl ToArgs for ChangelogCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("changelog".into());
    }
}

/// `agy help [subcommand]` — show help for subcommands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelpCommand {
    /// Optional subcommand to show help for.
    pub subcommand: Option<String>,
}

impl HelpCommand {
    /// `agy help <subcommand>`.
    #[must_use]
    pub fn for_subcommand(subcommand: impl Into<String>) -> Self {
        Self {
            subcommand: Some(subcommand.into()),
        }
    }
}

impl ToArgs for HelpCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("help".into());
        if let Some(subcommand) = &self.subcommand {
            args.push(subcommand.into());
        }
    }
}
