//! Informational and maintenance subcommands.
//!
//! `models`, `about`, `update`, `generate-rule` (alias `rule`), and the shell
//! integration installers take no options beyond `--help` in the pinned CLI,
//! so they are modeled as bare command tokens.

use std::ffi::OsString;

use crate::args::ToArgs;

/// `cursor-agent models` — list available models for this account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsCommand;

impl ToArgs for ModelsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("models".into());
    }
}

/// `cursor-agent about` — display version, system, and account information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AboutCommand;

impl ToArgs for AboutCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("about".into());
    }
}

/// `cursor-agent update` — update Cursor Agent to the latest version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCommand;

impl ToArgs for UpdateCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("update".into());
    }
}

/// `cursor-agent generate-rule` (alias `rule`) — generate a new Cursor rule
/// with interactive prompts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerateRuleCommand;

impl ToArgs for GenerateRuleCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("generate-rule".into());
    }
}

/// `cursor-agent install-shell-integration` — install shell integration to
/// `~/.zshrc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallShellIntegrationCommand;

impl ToArgs for InstallShellIntegrationCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("install-shell-integration".into());
    }
}

/// `cursor-agent uninstall-shell-integration` — remove shell integration from
/// `~/.zshrc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallShellIntegrationCommand;

impl ToArgs for UninstallShellIntegrationCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("uninstall-shell-integration".into());
    }
}
