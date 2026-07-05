//! `copilot [OPTIONS]` — the root run command.
//!
//! With no subcommand, `copilot` starts an interactive session, or — when a
//! prompt is supplied via `-p/--prompt` — runs one non-interactive turn and
//! exits. There is **no `suggest` subcommand** (that was a `gh copilot` relic);
//! the root command *is* the run. `--output-format json` makes the run emit the
//! JSONL event stream [`RunOutput`](crate::RunOutput) parses; select it with
//! [`RunCommand::json`].

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    ToArgs, push_attached, push_each, push_each_attached, push_enum, push_flag, push_opt,
    push_opt_path, push_paths,
};
use crate::values::{LogLevel, Mode, OutputFormat, ReasoningEffort, SessionSelector, Toggle};

/// `--share[=path]` — write a session transcript after a non-interactive run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareTarget {
    /// The bare flag (`--share`): Copilot's default `./copilot-session-<id>.md`.
    Default,
    /// The attached form (`--share=<path>`): an explicit markdown path.
    Path(PathBuf),
}

/// The full option surface of the root `copilot` run.
///
/// Every field maps to one Copilot flag; the booleans mirror Copilot's
/// independent on/off switches. Options render in a stable order so argv
/// snapshots are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    // ----- prompt / session ------------------------------------------------
    /// `-p, --prompt <text>`: run one non-interactive turn and exit.
    pub prompt: Option<String>,
    /// `-i, --interactive <prompt>`: start interactive mode, auto-running this.
    pub interactive: Option<String>,
    /// `-n, --name <name>`: name for the new session.
    pub name: Option<String>,
    /// `--continue`: resume the most recent session.
    pub continue_session: bool,
    /// `--resume[=value]`: resume a previous session (picker or specific ref).
    pub resume: Option<SessionSelector>,
    /// `--connect[=sessionId]`: connect directly to a remote session.
    pub connect: Option<SessionSelector>,

    // ----- model / reasoning ----------------------------------------------
    /// `--model <model>`.
    pub model: Option<String>,
    /// `--agent <agent>`: use a custom agent.
    pub agent: Option<String>,
    /// `--reasoning-effort <level>` (alias `--effort`).
    pub reasoning_effort: Option<ReasoningEffort>,
    /// `--enable-reasoning-summaries`.
    pub enable_reasoning_summaries: bool,

    // ----- mode ------------------------------------------------------------
    /// `--mode <mode>`: the initial agent mode.
    pub mode: Option<Mode>,
    /// `--plan`: start in plan mode.
    pub plan: bool,
    /// `--autopilot`: start in autopilot mode.
    pub autopilot: bool,
    /// `--max-autopilot-continues <count>`.
    pub max_autopilot_continues: Option<u32>,
    /// `--acp`: start as an Agent Client Protocol server.
    pub acp: bool,

    // ----- directories / files --------------------------------------------
    /// `-C <directory>`: change the working directory first.
    pub cd: Option<PathBuf>,
    /// `--add-dir <directory>` (repeatable): extend the allowed file set.
    pub add_dirs: Vec<PathBuf>,
    /// `--attachment <path>` (repeatable): attach files to the initial prompt.
    pub attachments: Vec<PathBuf>,
    /// `--plugin-dir <directory>` (repeatable): load a plugin from a directory.
    pub plugin_dirs: Vec<PathBuf>,
    /// `--log-dir <directory>`.
    pub log_dir: Option<PathBuf>,
    /// `--disallow-temp-dir`.
    pub disallow_temp_dir: bool,

    // ----- permissions -----------------------------------------------------
    /// `--allow-all`: enable every permission.
    pub allow_all: bool,
    /// `--yolo`: alias for `--allow-all`.
    pub yolo: bool,
    /// `--allow-all-tools`: run every tool without confirmation.
    pub allow_all_tools: bool,
    /// `--allow-all-paths`: disable file-path verification.
    pub allow_all_paths: bool,
    /// `--allow-all-urls`: allow every URL without confirmation.
    pub allow_all_urls: bool,
    /// `--allow-tool[=tools...]` (repeatable).
    pub allow_tools: Vec<String>,
    /// `--deny-tool[=tools...]` (repeatable).
    pub deny_tools: Vec<String>,
    /// `--allow-url[=urls...]` (repeatable).
    pub allow_urls: Vec<String>,
    /// `--deny-url[=urls...]` (repeatable).
    pub deny_urls: Vec<String>,
    /// `--available-tools[=tools...]` (repeatable): the only available tools.
    pub available_tools: Vec<String>,
    /// `--excluded-tools[=tools...]` (repeatable): tools hidden from the model.
    pub excluded_tools: Vec<String>,
    /// `--no-ask-user`: disable the `ask_user` tool.
    pub no_ask_user: bool,

    // ----- MCP / GitHub MCP ------------------------------------------------
    /// `--add-github-mcp-tool <tool>` (repeatable).
    pub add_github_mcp_tools: Vec<String>,
    /// `--add-github-mcp-toolset <toolset>` (repeatable).
    pub add_github_mcp_toolsets: Vec<String>,
    /// `--enable-all-github-mcp-tools`.
    pub enable_all_github_mcp_tools: bool,
    /// `--additional-mcp-config <json>` (repeatable): JSON string or `@file`.
    pub additional_mcp_config: Vec<String>,
    /// `--disable-mcp-server <server-name>` (repeatable).
    pub disable_mcp_servers: Vec<String>,
    /// `--disable-builtin-mcps`.
    pub disable_builtin_mcps: bool,

    // ----- output / UI -----------------------------------------------------
    /// `--output-format <format>`.
    pub output_format: Option<OutputFormat>,
    /// `-s, --silent`: output only the agent response (scripting with `-p`).
    pub silent: bool,
    /// `--no-color`.
    pub no_color: bool,
    /// `--banner`: show the startup banner.
    pub banner: bool,
    /// `--screen-reader`.
    pub screen_reader: bool,
    /// `--plain-diff`.
    pub plain_diff: bool,
    /// `--mouse[=value]`: mouse support in alt-screen mode.
    pub mouse: Option<Toggle>,
    /// `--no-mouse`.
    pub no_mouse: bool,
    /// `--stream <mode>`: enable or disable streaming.
    pub stream: Option<Toggle>,
    /// `--log-level <level>`.
    pub log_level: Option<LogLevel>,

    // ----- instructions / experimental ------------------------------------
    /// `--no-custom-instructions`: skip AGENTS.md and related files.
    pub no_custom_instructions: bool,
    /// `--experimental`.
    pub experimental: bool,
    /// `--no-experimental`.
    pub no_experimental: bool,
    /// `--bash-env[=value]`: BASH_ENV support for bash shells.
    pub bash_env: Option<Toggle>,
    /// `--no-bash-env`.
    pub no_bash_env: bool,
    /// `--secret-env-vars[=vars...]` (repeatable): redacted env-var names.
    pub secret_env_vars: Vec<String>,

    // ----- remote / update / sharing --------------------------------------
    /// `--remote`: enable remote control from GitHub web and mobile.
    pub remote: bool,
    /// `--no-remote`.
    pub no_remote: bool,
    /// `--no-auto-update`.
    pub no_auto_update: bool,
    /// `--share[=path]`: share the session to markdown after a `-p` run.
    pub share: Option<ShareTarget>,
    /// `--share-gist`: share the session to a secret gist after a `-p` run.
    pub share_gist: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        // prompt / session
        push_opt(args, "--prompt", self.prompt.as_deref());
        push_opt(args, "--interactive", self.interactive.as_deref());
        push_opt(args, "--name", self.name.as_deref());
        push_flag(args, self.continue_session, "--continue");
        render_selector(args, "--resume", self.resume.as_ref());
        render_selector(args, "--connect", self.connect.as_ref());

        // model / reasoning
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--agent", self.agent.as_deref());
        push_enum(
            args,
            "--reasoning-effort",
            self.reasoning_effort.map(ReasoningEffort::as_str),
        );
        push_flag(
            args,
            self.enable_reasoning_summaries,
            "--enable-reasoning-summaries",
        );

        // mode
        push_enum(args, "--mode", self.mode.map(Mode::as_str));
        push_flag(args, self.plan, "--plan");
        push_flag(args, self.autopilot, "--autopilot");
        if let Some(count) = self.max_autopilot_continues {
            args.push("--max-autopilot-continues".into());
            args.push(count.to_string().into());
        }
        push_flag(args, self.acp, "--acp");

        // directories / files
        push_opt_path(args, "-C", self.cd.as_deref());
        push_paths(args, "--add-dir", &self.add_dirs);
        push_paths(args, "--attachment", &self.attachments);
        push_paths(args, "--plugin-dir", &self.plugin_dirs);
        push_opt_path(args, "--log-dir", self.log_dir.as_deref());
        push_flag(args, self.disallow_temp_dir, "--disallow-temp-dir");

        // permissions
        push_flag(args, self.allow_all, "--allow-all");
        push_flag(args, self.yolo, "--yolo");
        push_flag(args, self.allow_all_tools, "--allow-all-tools");
        push_flag(args, self.allow_all_paths, "--allow-all-paths");
        push_flag(args, self.allow_all_urls, "--allow-all-urls");
        push_each_attached(args, "--allow-tool", &self.allow_tools);
        push_each_attached(args, "--deny-tool", &self.deny_tools);
        push_each_attached(args, "--allow-url", &self.allow_urls);
        push_each_attached(args, "--deny-url", &self.deny_urls);
        push_each_attached(args, "--available-tools", &self.available_tools);
        push_each_attached(args, "--excluded-tools", &self.excluded_tools);
        push_flag(args, self.no_ask_user, "--no-ask-user");

        // MCP / GitHub MCP
        push_each(args, "--add-github-mcp-tool", &self.add_github_mcp_tools);
        push_each(
            args,
            "--add-github-mcp-toolset",
            &self.add_github_mcp_toolsets,
        );
        push_flag(
            args,
            self.enable_all_github_mcp_tools,
            "--enable-all-github-mcp-tools",
        );
        push_each(args, "--additional-mcp-config", &self.additional_mcp_config);
        push_each(args, "--disable-mcp-server", &self.disable_mcp_servers);
        push_flag(args, self.disable_builtin_mcps, "--disable-builtin-mcps");

        // output / UI
        push_enum(
            args,
            "--output-format",
            self.output_format.map(OutputFormat::as_str),
        );
        push_flag(args, self.silent, "--silent");
        push_flag(args, self.no_color, "--no-color");
        push_flag(args, self.banner, "--banner");
        push_flag(args, self.screen_reader, "--screen-reader");
        push_flag(args, self.plain_diff, "--plain-diff");
        push_attached(args, "--mouse", self.mouse.map(Toggle::as_str));
        push_flag(args, self.no_mouse, "--no-mouse");
        push_enum(args, "--stream", self.stream.map(Toggle::as_str));
        push_enum(args, "--log-level", self.log_level.map(LogLevel::as_str));

        // instructions / experimental
        push_flag(args, self.no_custom_instructions, "--no-custom-instructions");
        push_flag(args, self.experimental, "--experimental");
        push_flag(args, self.no_experimental, "--no-experimental");
        push_attached(args, "--bash-env", self.bash_env.map(Toggle::as_str));
        push_flag(args, self.no_bash_env, "--no-bash-env");
        push_each_attached(args, "--secret-env-vars", &self.secret_env_vars);

        // remote / update / sharing
        push_flag(args, self.remote, "--remote");
        push_flag(args, self.no_remote, "--no-remote");
        push_flag(args, self.no_auto_update, "--no-auto-update");
        render_share(args, self.share.as_ref());
        push_flag(args, self.share_gist, "--share-gist");
    }
}

fn render_selector(args: &mut Vec<OsString>, flag: &str, selector: Option<&SessionSelector>) {
    match selector {
        None => {}
        Some(SessionSelector::Prompt) => args.push(flag.into()),
        Some(SessionSelector::Ref(value)) => args.push(format!("{flag}={value}").into()),
    }
}

fn render_share(args: &mut Vec<OsString>, share: Option<&ShareTarget>) {
    match share {
        None => {}
        Some(ShareTarget::Default) => args.push("--share".into()),
        Some(ShareTarget::Path(path)) => {
            let mut arg = OsString::from("--share=");
            arg.push(path);
            args.push(arg);
        }
    }
}

/// `copilot [OPTIONS]` — the root run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// The full root-run option surface.
    pub options: RunOptions,
}

impl RunCommand {
    /// Build a non-interactive run seeded with a `-p/--prompt` value.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            options: RunOptions {
                prompt: Some(prompt.into()),
                ..RunOptions::default()
            },
        }
    }

    /// Select `--output-format json`, yielding a typed
    /// [`RunOutput`](crate::RunOutput).
    ///
    /// This forces `output_format` to [`OutputFormat::Json`] regardless of any
    /// prior value so the run is guaranteed to emit the JSONL event stream.
    #[must_use]
    pub fn json(mut self) -> JsonRunCommand {
        self.options.output_format = Some(OutputFormat::Json);
        JsonRunCommand { command: self }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.options.render(args);
    }
}

/// `copilot --output-format json [OPTIONS]`.
///
/// [`Copilot::execute`](crate::Copilot::execute) returns
/// [`RunOutput`](crate::RunOutput); [`Copilot::stream`](crate::Copilot::stream)
/// reads its events incrementally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonRunCommand {
    command: RunCommand,
}

impl JsonRunCommand {
    /// Borrow the underlying run configuration.
    #[must_use]
    pub const fn command(&self) -> &RunCommand {
        &self.command
    }

    /// Return the underlying run configuration.
    #[must_use]
    pub fn into_command(self) -> RunCommand {
        self.command
    }
}

impl ToArgs for JsonRunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.write_args(args);
    }
}
