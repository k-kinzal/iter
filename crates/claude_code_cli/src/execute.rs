use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    ToArgs, push_each, push_enum, push_flag, push_joined, push_opt, push_opt_path,
    push_optional_value, push_pair, push_pair_os, push_paths, push_positional_boundary,
    push_setting_sources,
};
use crate::values::{
    BooleanChoice, Chrome, EffortLevel, FileResource, InputFormat, MaxBudgetUsd, OptionalValue,
    OutputFormat, PermissionMode, SettingSource, Switch, TmuxMode, ToolSet,
};

/// Top-level `claude [options] [prompt]` execution builder.
///
/// This is not a `claude execute` subcommand. It models Claude Code's root
/// prompt form and lets the selected output mode determine the Rust return
/// type of [`crate::ClaudeCode::execute`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecuteCommand {
    /// Optional prompt positional argument.
    pub prompt: Option<String>,
    /// `--add-dir`.
    pub add_dirs: Vec<PathBuf>,
    /// `--agent`.
    pub agent: Option<String>,
    /// `--agents`.
    pub agents_json: Option<String>,
    /// `--allow-dangerously-skip-permissions`.
    pub allow_dangerously_skip_permissions: Switch,
    /// `--allowed-tools`.
    pub allowed_tools: Vec<String>,
    /// `--append-system-prompt`.
    pub append_system_prompt: Option<String>,
    /// Hidden `--append-system-prompt-file`.
    pub append_system_prompt_file: Option<PathBuf>,
    /// `--bare`.
    pub bare: Switch,
    /// `--betas`.
    pub betas: Vec<String>,
    /// `--brief`.
    pub brief: Switch,
    /// `--chrome` / `--no-chrome`.
    pub chrome: Option<Chrome>,
    /// `--continue`.
    pub continue_latest: Switch,
    /// `--dangerously-skip-permissions`.
    pub dangerously_skip_permissions: Switch,
    /// `--debug [filter]`.
    pub debug: Option<OptionalValue<String>>,
    /// `--debug-file`.
    pub debug_file: Option<PathBuf>,
    /// `--disable-slash-commands`.
    pub disable_slash_commands: Switch,
    /// `--disallowed-tools`.
    pub disallowed_tools: Vec<String>,
    /// `--effort`.
    pub effort: Option<EffortLevel>,
    /// `--exclude-dynamic-system-prompt-sections`.
    pub exclude_dynamic_system_prompt_sections: Switch,
    /// `--fallback-model`.
    ///
    /// Claude Code 2.1.178 `claude --help` documents this as a comma-separated
    /// model list for a single flag occurrence.
    pub fallback_models: Vec<String>,
    /// `--file`.
    pub files: Vec<FileResource>,
    /// `--fork-session`.
    pub fork_session: Switch,
    /// `--from-pr [value]`.
    pub from_pr: Option<OptionalValue<String>>,
    /// `--ide`.
    pub ide: Switch,
    /// `--include-hook-events`.
    pub include_hook_events: Switch,
    /// `--include-partial-messages`.
    pub include_partial_messages: Switch,
    /// `--input-format`.
    pub input_format: Option<InputFormat>,
    /// `--json-schema`.
    pub json_schema: Option<String>,
    /// `--max-budget-usd`.
    pub max_budget_usd: Option<MaxBudgetUsd>,
    /// `--mcp-config`.
    pub mcp_configs: Vec<String>,
    /// `--mcp-debug`.
    pub mcp_debug: Switch,
    /// `--model`.
    pub model: Option<String>,
    /// `--name`.
    pub name: Option<String>,
    /// `--no-session-persistence`.
    pub no_session_persistence: Switch,
    /// `--permission-mode`.
    pub permission_mode: Option<PermissionMode>,
    /// `--plugin-dir`.
    pub plugin_dirs: Vec<PathBuf>,
    /// `--plugin-url`.
    pub plugin_urls: Vec<String>,
    /// `--prompt-suggestions [value]`.
    pub prompt_suggestions: Option<OptionalValue<BooleanChoice>>,
    /// `--remote-control [name]`.
    pub remote_control: Option<OptionalValue<String>>,
    /// `--remote-control-session-name-prefix`.
    pub remote_control_session_name_prefix: Option<String>,
    /// `--replay-user-messages`.
    pub replay_user_messages: Switch,
    /// `--resume [value]`.
    pub resume: Option<OptionalValue<String>>,
    /// `--safe-mode`.
    pub safe_mode: Switch,
    /// `--session-id`.
    pub session_id: Option<uuid::Uuid>,
    /// `--setting-sources`.
    pub setting_sources: Vec<SettingSource>,
    /// `--settings`.
    pub settings: Option<String>,
    /// `--strict-mcp-config`.
    pub strict_mcp_config: Switch,
    /// `--system-prompt`.
    pub system_prompt: Option<String>,
    /// Hidden `--system-prompt-file`.
    pub system_prompt_file: Option<PathBuf>,
    /// `--tmux`.
    pub tmux: Option<OptionalValue<TmuxMode>>,
    /// `--tools`.
    pub tools: Option<ToolSet>,
    /// `--verbose`.
    pub verbose: Switch,
    /// `--version`.
    pub version: Switch,
    /// `--worktree [name]`.
    pub worktree: Option<OptionalValue<String>>,
}

impl ExecuteCommand {
    /// Build a root Claude Code execution with a prompt positional argument.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }

    /// Select `--print --output-format text`.
    #[must_use]
    pub fn text(self) -> TextExecuteCommand {
        TextExecuteCommand { command: self }
    }

    /// Select `--print --output-format json`.
    #[must_use]
    pub fn json(self) -> JsonExecuteCommand {
        JsonExecuteCommand { command: self }
    }

    /// Select `--print --output-format stream-json`.
    ///
    /// Claude Code 2.1.178 requires `--verbose` with stream-json output; the
    /// rendered command includes it automatically.
    #[must_use]
    pub fn stream_json(self) -> StreamJsonExecuteCommand {
        StreamJsonExecuteCommand { command: self }
    }

    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        self.render_with(args, None, Switch::Off, false);
    }

    pub(crate) fn render_with(
        &self,
        args: &mut Vec<OsString>,
        output_format: Option<OutputFormat>,
        print: Switch,
        force_verbose: bool,
    ) {
        push_paths(args, "--add-dir", &self.add_dirs);
        push_opt(args, "--agent", self.agent.as_deref());
        push_opt(args, "--agents", self.agents_json.as_deref());
        push_flag(
            args,
            self.allow_dangerously_skip_permissions,
            "--allow-dangerously-skip-permissions",
        );
        push_each(args, "--allowed-tools", &self.allowed_tools);
        push_opt(
            args,
            "--append-system-prompt",
            self.append_system_prompt.as_deref(),
        );
        push_opt_path(
            args,
            "--append-system-prompt-file",
            self.append_system_prompt_file.as_deref(),
        );
        push_flag(args, self.bare, "--bare");
        push_each(args, "--betas", &self.betas);
        push_flag(args, self.brief, "--brief");
        if let Some(chrome) = self.chrome {
            args.push(
                match chrome {
                    Chrome::Enable => "--chrome",
                    Chrome::Disable => "--no-chrome",
                }
                .into(),
            );
        }
        push_flag(args, self.continue_latest, "--continue");
        push_flag(
            args,
            self.dangerously_skip_permissions,
            "--dangerously-skip-permissions",
        );
        push_optional_value(args, "--debug", self.debug.as_ref(), Clone::clone);
        push_opt_path(args, "--debug-file", self.debug_file.as_deref());
        push_flag(
            args,
            self.disable_slash_commands,
            "--disable-slash-commands",
        );
        push_each(args, "--disallowed-tools", &self.disallowed_tools);
        push_enum(args, "--effort", self.effort.map(EffortLevel::as_str));
        push_flag(
            args,
            self.exclude_dynamic_system_prompt_sections,
            "--exclude-dynamic-system-prompt-sections",
        );
        push_joined(args, "--fallback-model", &self.fallback_models);
        for file in &self.files {
            push_pair_os(args, "--file", file.value());
        }
        push_flag(args, self.fork_session, "--fork-session");
        push_optional_value(args, "--from-pr", self.from_pr.as_ref(), Clone::clone);
        push_flag(args, self.ide, "--ide");
        push_flag(args, self.include_hook_events, "--include-hook-events");
        push_flag(
            args,
            self.include_partial_messages,
            "--include-partial-messages",
        );
        push_enum(
            args,
            "--input-format",
            self.input_format.map(InputFormat::as_str),
        );
        push_opt(args, "--json-schema", self.json_schema.as_deref());
        push_opt(
            args,
            "--max-budget-usd",
            self.max_budget_usd.map(MaxBudgetUsd::render).as_deref(),
        );
        push_each(args, "--mcp-config", &self.mcp_configs);
        push_flag(args, self.mcp_debug, "--mcp-debug");
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--name", self.name.as_deref());
        push_flag(
            args,
            self.no_session_persistence,
            "--no-session-persistence",
        );
        push_enum(
            args,
            "--output-format",
            output_format.map(OutputFormat::as_str),
        );
        push_enum(
            args,
            "--permission-mode",
            self.permission_mode.map(PermissionMode::as_str),
        );
        push_paths(args, "--plugin-dir", &self.plugin_dirs);
        push_each(args, "--plugin-url", &self.plugin_urls);
        push_flag(args, print, "--print");
        push_optional_value(
            args,
            "--prompt-suggestions",
            self.prompt_suggestions.as_ref(),
            |v| v.as_str().to_owned(),
        );
        push_optional_value(
            args,
            "--remote-control",
            self.remote_control.as_ref(),
            Clone::clone,
        );
        push_opt(
            args,
            "--remote-control-session-name-prefix",
            self.remote_control_session_name_prefix.as_deref(),
        );
        push_flag(args, self.replay_user_messages, "--replay-user-messages");
        push_optional_value(args, "--resume", self.resume.as_ref(), Clone::clone);
        push_flag(args, self.safe_mode, "--safe-mode");
        push_opt(
            args,
            "--session-id",
            self.session_id.map(|value| value.to_string()).as_deref(),
        );
        push_setting_sources(args, &self.setting_sources);
        push_opt(args, "--settings", self.settings.as_deref());
        push_flag(args, self.strict_mcp_config, "--strict-mcp-config");
        push_opt(args, "--system-prompt", self.system_prompt.as_deref());
        push_opt_path(
            args,
            "--system-prompt-file",
            self.system_prompt_file.as_deref(),
        );
        push_optional_value(args, "--tmux", self.tmux.as_ref(), |v| {
            v.as_str().to_owned()
        });
        if let Some(tools) = &self.tools {
            push_pair(args, "--tools", tools.value());
        }
        push_flag(args, self.verbose, "--verbose");
        if force_verbose && !self.verbose.is_on() {
            args.push("--verbose".into());
        }
        push_flag(args, self.version, "--version");
        push_optional_value(args, "--worktree", self.worktree.as_ref(), Clone::clone);
        if let Some(prompt) = &self.prompt {
            push_positional_boundary(args);
            args.push(prompt.into());
        }
    }
}

impl ToArgs for ExecuteCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args);
    }
}

/// `ExecuteCommand` using `--print --output-format text`.
///
/// [`ClaudeCode::execute`](crate::ClaudeCode::execute) returns
/// [`TextOutput`](crate::TextOutput).
#[derive(Debug, Clone, PartialEq)]
pub struct TextExecuteCommand {
    command: ExecuteCommand,
}

impl TextExecuteCommand {
    /// Borrow the prompt command configuration.
    #[must_use]
    pub const fn command(&self) -> &ExecuteCommand {
        &self.command
    }

    /// Return the prompt command configuration.
    #[must_use]
    pub fn into_command(self) -> ExecuteCommand {
        self.command
    }

    /// Render this command into argv entries after the `claude` executable.
    #[must_use]
    pub fn to_args(&self) -> Vec<OsString> {
        ToArgs::to_args(self)
    }
}

impl ToArgs for TextExecuteCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command
            .render_with(args, Some(OutputFormat::Text), Switch::On, false);
    }
}

/// `ExecuteCommand` using `--print --output-format json`.
///
/// [`ClaudeCode::execute`](crate::ClaudeCode::execute) returns
/// [`JsonOutput`](crate::JsonOutput).
#[derive(Debug, Clone, PartialEq)]
pub struct JsonExecuteCommand {
    command: ExecuteCommand,
}

impl JsonExecuteCommand {
    /// Borrow the prompt command configuration.
    #[must_use]
    pub const fn command(&self) -> &ExecuteCommand {
        &self.command
    }

    /// Return the prompt command configuration.
    #[must_use]
    pub fn into_command(self) -> ExecuteCommand {
        self.command
    }

    /// Render this command into argv entries after the `claude` executable.
    #[must_use]
    pub fn to_args(&self) -> Vec<OsString> {
        ToArgs::to_args(self)
    }
}

impl ToArgs for JsonExecuteCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command
            .render_with(args, Some(OutputFormat::Json), Switch::On, false);
    }
}

/// `ExecuteCommand` using `--print --output-format stream-json`.
///
/// [`ClaudeCode::execute`](crate::ClaudeCode::execute) and
/// [`ClaudeCode::stream`](crate::ClaudeCode::stream) both return
/// [`StreamOutput`](crate::StreamOutput).
#[derive(Debug, Clone, PartialEq)]
pub struct StreamJsonExecuteCommand {
    command: ExecuteCommand,
}

impl StreamJsonExecuteCommand {
    /// Borrow the prompt command configuration.
    #[must_use]
    pub const fn command(&self) -> &ExecuteCommand {
        &self.command
    }

    /// Return the prompt command configuration.
    #[must_use]
    pub fn into_command(self) -> ExecuteCommand {
        self.command
    }

    /// Render this command into argv entries after the `claude` executable.
    #[must_use]
    pub fn to_args(&self) -> Vec<OsString> {
        ToArgs::to_args(self)
    }
}

impl ToArgs for StreamJsonExecuteCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command
            .render_with(args, Some(OutputFormat::StreamJson), Switch::On, true);
    }
}
