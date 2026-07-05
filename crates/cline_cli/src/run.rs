//! `cline [OPTIONS] [PROMPT]` — the root prompt run.
//!
//! With no subcommand, Cline runs the agent on the positional `PROMPT`. The
//! prompt is a **positional argument** — Cline `3.0.23` has no `--oneshot`
//! flag and reads nothing from stdin. The default disposition is act mode with
//! tool auto-approval enabled.
//!
//! The plain [`RunCommand`] renders without `--json` (Cline prints styled
//! text); call [`RunCommand::json`] to request the NDJSON run stream and a
//! typed [`RunOutput`](crate::RunOutput).

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    ToArgs, push_bool, push_enum, push_flag, push_opt, push_opt_num, push_opt_path,
};
use crate::values::{CompactionMode, ThinkingLevel};

/// Options for the root `cline` run (rendered before the prompt positional).
///
/// These render in a stable order so argv snapshots are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `-p, --plan`: run in plan mode instead of the default act mode.
    pub plan: bool,
    /// `--auto-approve <boolean>`: set tool auto-approval for all tools
    /// (Cline's default is `true`).
    pub auto_approve: Option<bool>,
    /// `-c, --cwd <path>`: working directory.
    pub cwd: Option<PathBuf>,
    /// `--thinking <level>`: reasoning-effort level.
    pub thinking: Option<ThinkingLevel>,
    /// `--compaction <mode>`: context-compaction mode.
    pub compaction: Option<CompactionMode>,
    /// `-i, --tui`: open the interactive terminal UI.
    pub tui: bool,
    /// `--id <session-id>`: resume an existing session by ID.
    pub id: Option<String>,
    /// `-P, --provider <id>`: provider id (Cline's default is `cline`).
    pub provider: Option<String>,
    /// `-k, --key <api-key>`: API key override for this run.
    pub key: Option<String>,
    /// `-m, --model <model-id>`: model to use for the session.
    pub model: Option<String>,
    /// `-s, --system <system-prompt>`: override the default system prompt.
    pub system: Option<String>,
    /// `-z, --zen`: start a session that runs in the background hub.
    pub zen: bool,
    /// `--retries [value]`: maximum consecutive mistakes before exiting
    /// (Cline's default is `6`).
    pub retries: Option<u32>,
    /// `-t, --timeout <seconds>`: optional timeout (`0` = no timeout).
    pub timeout: Option<u64>,
    /// `--acp`: run in Agent Client Protocol mode for editor integration.
    pub acp: bool,
    /// `--config <path>`: configuration directory.
    pub config: Option<PathBuf>,
    /// `--data-dir <path>`: use isolated local state at this directory.
    pub data_dir: Option<PathBuf>,
    /// `--hooks-dir <path>`: directory of additional runtime hooks.
    pub hooks_dir: Option<PathBuf>,
    /// `--worktree`: auto-create a detached git worktree and run there.
    pub worktree: bool,
    /// `--update`: check for updates and install if available.
    pub update: bool,
    /// `--kanban`: run the kanban app.
    pub kanban: bool,
    /// `-v, --verbose`: show verbose output.
    pub verbose: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.plan, "--plan");
        push_bool(args, "--auto-approve", self.auto_approve);
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_enum(args, "--thinking", self.thinking.map(ThinkingLevel::as_str));
        push_enum(
            args,
            "--compaction",
            self.compaction.map(CompactionMode::as_str),
        );
        push_flag(args, self.tui, "--tui");
        push_opt(args, "--id", self.id.as_deref());
        push_opt(args, "--provider", self.provider.as_deref());
        push_opt(args, "--key", self.key.as_deref());
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--system", self.system.as_deref());
        push_flag(args, self.zen, "--zen");
        push_opt_num(args, "--retries", self.retries);
        push_opt_num(args, "--timeout", self.timeout);
        push_flag(args, self.acp, "--acp");
        push_opt_path(args, "--config", self.config.as_deref());
        push_opt_path(args, "--data-dir", self.data_dir.as_deref());
        push_opt_path(args, "--hooks-dir", self.hooks_dir.as_deref());
        push_flag(args, self.worktree, "--worktree");
        push_flag(args, self.update, "--update");
        push_flag(args, self.kanban, "--kanban");
        push_flag(args, self.verbose, "--verbose");
    }
}

/// `cline [OPTIONS] [PROMPT]` — the root prompt run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// Root-run options rendered before the prompt.
    pub options: RunOptions,
    /// Optional prompt positional. `None` launches Cline with no seed prompt.
    pub prompt: Option<String>,
}

impl RunCommand {
    /// Build a run with a prompt positional argument.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }

    /// Select `cline --json`, yielding a typed [`RunOutput`](crate::RunOutput).
    #[must_use]
    pub fn json(self) -> JsonRunCommand {
        JsonRunCommand { command: self }
    }

    fn render(&self, args: &mut Vec<OsString>, json: bool) {
        // `--json` renders first (the output-format toggle), then the managed
        // options, then the prompt positional last.
        push_flag(args, json, "--json");
        self.options.render(args);
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args, false);
    }
}

/// `cline --json [OPTIONS] [PROMPT]`.
///
/// [`Cline::execute`](crate::Cline::execute) returns
/// [`RunOutput`](crate::RunOutput).
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
        self.command.render(args, true);
    }
}
