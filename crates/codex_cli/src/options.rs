//! Options shared by Codex's root run and its `exec` subcommand.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{push_each, push_enum, push_flag, push_opt, push_paths};
use crate::values::{ConfigOverride, LocalProvider, SandboxMode};

/// The `-c/--config`, `--enable`, and `--disable` options every Codex
/// subcommand accepts.
///
/// Codex threads these through even to management subcommands (`mcp`,
/// `plugin`, `login`, `doctor`, …) so they share this small struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalConfig {
    /// `-c, --config key=value` (repeatable).
    pub config: Vec<ConfigOverride>,
    /// `--enable <FEATURE>` (repeatable).
    pub enable: Vec<String>,
    /// `--disable <FEATURE>` (repeatable).
    pub disable: Vec<String>,
}

impl GlobalConfig {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        for override_ in &self.config {
            args.push("--config".into());
            args.push(override_.render().into());
        }
        push_each(args, "--enable", &self.enable);
        push_each(args, "--disable", &self.disable);
    }
}

/// Configuration and model-selection options common to `codex [PROMPT]` and
/// `codex exec [PROMPT]`.
///
/// These render in a stable order so argv snapshots are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommonConfig {
    /// `-c, --config key=value` (repeatable).
    pub config: Vec<ConfigOverride>,
    /// `--enable <FEATURE>` (repeatable).
    pub enable: Vec<String>,
    /// `--disable <FEATURE>` (repeatable).
    pub disable: Vec<String>,
    /// `--strict-config`.
    pub strict_config: bool,
    /// `-i, --image <FILE>` (repeatable).
    pub images: Vec<PathBuf>,
    /// `-m, --model <MODEL>`.
    pub model: Option<String>,
    /// `--oss`.
    pub oss: bool,
    /// `--local-provider <OSS_PROVIDER>`.
    pub local_provider: Option<LocalProvider>,
    /// `-p, --profile <CONFIG_PROFILE_V2>`.
    pub profile: Option<String>,
    /// `-s, --sandbox <SANDBOX_MODE>`.
    pub sandbox: Option<SandboxMode>,
    /// `--dangerously-bypass-approvals-and-sandbox`.
    pub dangerously_bypass_approvals_and_sandbox: bool,
    /// `--dangerously-bypass-hook-trust`.
    pub dangerously_bypass_hook_trust: bool,
    /// `-C, --cd <DIR>`.
    pub cd: Option<PathBuf>,
    /// `--add-dir <DIR>` (repeatable).
    pub add_dirs: Vec<PathBuf>,
}

impl CommonConfig {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        for override_ in &self.config {
            args.push("--config".into());
            args.push(override_.render().into());
        }
        push_each(args, "--enable", &self.enable);
        push_each(args, "--disable", &self.disable);
        push_flag(args, self.strict_config, "--strict-config");
        push_paths(args, "--image", &self.images);
        push_opt(args, "--model", self.model.as_deref());
        push_flag(args, self.oss, "--oss");
        push_enum(
            args,
            "--local-provider",
            self.local_provider.map(LocalProvider::as_str),
        );
        push_opt(args, "--profile", self.profile.as_deref());
        push_enum(args, "--sandbox", self.sandbox.map(SandboxMode::as_str));
        push_flag(
            args,
            self.dangerously_bypass_approvals_and_sandbox,
            "--dangerously-bypass-approvals-and-sandbox",
        );
        push_flag(
            args,
            self.dangerously_bypass_hook_trust,
            "--dangerously-bypass-hook-trust",
        );
        if let Some(cd) = &self.cd {
            args.push("--cd".into());
            args.push(cd.into());
        }
        push_paths(args, "--add-dir", &self.add_dirs);
    }
}
