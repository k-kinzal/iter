use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    push_each, push_enum, push_flag, push_opt, push_opt_path, push_paths, push_setting_sources,
};
use crate::values::{EffortLevel, PermissionMode, SettingSource, Switch};

/// `claude agents`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agents {
    /// `--add-dir`.
    pub add_dirs: Vec<PathBuf>,
    /// `--agent`.
    pub agent: Option<String>,
    /// `--all`.
    pub all: Switch,
    /// `--allow-dangerously-skip-permissions`.
    pub allow_dangerously_skip_permissions: Switch,
    /// `--cwd`.
    pub cwd: Option<PathBuf>,
    /// `--dangerously-skip-permissions`.
    pub dangerously_skip_permissions: Switch,
    /// `--effort`.
    pub effort: Option<EffortLevel>,
    /// `--json`.
    pub json: Switch,
    /// `--mcp-config`.
    pub mcp_configs: Vec<String>,
    /// `--model`.
    pub model: Option<String>,
    /// `--permission-mode`.
    pub permission_mode: Option<PermissionMode>,
    /// `--plugin-dir`.
    pub plugin_dirs: Vec<PathBuf>,
    /// `--setting-sources`.
    pub setting_sources: Vec<SettingSource>,
    /// `--settings`.
    pub settings: Option<String>,
    /// `--strict-mcp-config`.
    pub strict_mcp_config: Switch,
}

impl Agents {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_paths(args, "--add-dir", &self.add_dirs);
        push_opt(args, "--agent", self.agent.as_deref());
        push_flag(args, self.all, "--all");
        push_flag(
            args,
            self.allow_dangerously_skip_permissions,
            "--allow-dangerously-skip-permissions",
        );
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_flag(
            args,
            self.dangerously_skip_permissions,
            "--dangerously-skip-permissions",
        );
        push_enum(args, "--effort", self.effort.map(EffortLevel::as_str));
        push_flag(args, self.json, "--json");
        push_each(args, "--mcp-config", &self.mcp_configs);
        push_opt(args, "--model", self.model.as_deref());
        push_enum(
            args,
            "--permission-mode",
            self.permission_mode.map(PermissionMode::as_str),
        );
        push_paths(args, "--plugin-dir", &self.plugin_dirs);
        push_setting_sources(args, &self.setting_sources);
        push_opt(args, "--settings", self.settings.as_deref());
        push_flag(args, self.strict_mcp_config, "--strict-mcp-config");
    }
}
