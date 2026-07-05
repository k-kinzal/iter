//! `hermes chat [OPTIONS]` — the interactive chat session, and its quiet
//! programmatic mode.
//!
//! `chat -Q` (`--quiet`) is the second text-only agent entry point: it
//! suppresses the banner, spinner, and tool previews and prints only the final
//! response and session info. Pairing it with `-q <QUERY>` (`--query`) runs a
//! single non-interactive turn — the shape a script or CI job invokes.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_num, push_opt_path};
use crate::run::ContinueMode;

/// Options for `hermes chat`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatOptions {
    /// `--image <PATH>`: attach a local image to a single query.
    pub image: Option<PathBuf>,
    /// `-m` / `--model <MODEL>`: model to use.
    pub model: Option<String>,
    /// `-t` / `--toolsets <TOOLSETS>`: comma-separated toolsets to enable.
    pub toolsets: Option<String>,
    /// `-s` / `--skills <SKILLS>`: preload one or more skills.
    pub skills: Option<String>,
    /// `--provider <PROVIDER>`: inference provider.
    pub provider: Option<String>,
    /// `-v` / `--verbose`: verbose output.
    pub verbose: bool,
    /// `-Q` / `--quiet`: quiet mode for programmatic use.
    pub quiet: bool,
    /// `-r` / `--resume <SESSION_ID>`: resume a previous session by ID.
    pub resume: Option<String>,
    /// `-c` / `--continue [SESSION_NAME]`: resume a session by name, or the most
    /// recent when no name is given.
    pub continue_session: Option<ContinueMode>,
    /// `-w` / `--worktree`: run in an isolated git worktree.
    pub worktree: bool,
    /// `--accept-hooks`: auto-approve unseen shell hooks without a TTY prompt.
    pub accept_hooks: bool,
    /// `--checkpoints`: enable filesystem checkpoints before destructive file
    /// operations.
    pub checkpoints: bool,
    /// `--max-turns <N>`: maximum tool-calling iterations per turn.
    pub max_turns: Option<u32>,
    /// `--yolo`: bypass all dangerous command approval prompts.
    pub yolo: bool,
    /// `--pass-session-id`: include the session ID in the agent's system
    /// prompt.
    pub pass_session_id: bool,
    /// `--ignore-user-config`: ignore `~/.hermes/config.yaml`.
    pub ignore_user_config: bool,
    /// `--ignore-rules`: skip auto-injection of AGENTS.md, memory, and skills.
    pub ignore_rules: bool,
    /// `--safe-mode`: disable all customizations (implies the two ignores).
    pub safe_mode: bool,
    /// `--source <SOURCE>`: session source tag for filtering (default `cli`).
    pub source: Option<String>,
    /// `--tui`: launch the modern TUI instead of the classic REPL.
    pub tui: bool,
    /// `--cli`: force the classic prompt_toolkit REPL.
    pub cli: bool,
    /// `--dev`: with `--tui`, run the TypeScript sources via tsx.
    pub dev: bool,
}

impl ChatOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_opt_path(args, "--image", self.image.as_deref());
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--toolsets", self.toolsets.as_deref());
        push_opt(args, "--skills", self.skills.as_deref());
        push_opt(args, "--provider", self.provider.as_deref());
        push_flag(args, self.verbose, "--verbose");
        push_flag(args, self.quiet, "--quiet");
        push_opt(args, "--resume", self.resume.as_deref());
        match &self.continue_session {
            Some(ContinueMode::MostRecent) => args.push("--continue".into()),
            Some(ContinueMode::Named(name)) => {
                args.push("--continue".into());
                args.push(name.into());
            }
            None => {}
        }
        push_flag(args, self.worktree, "--worktree");
        push_flag(args, self.accept_hooks, "--accept-hooks");
        push_flag(args, self.checkpoints, "--checkpoints");
        push_opt_num(args, "--max-turns", self.max_turns);
        push_flag(args, self.yolo, "--yolo");
        push_flag(args, self.pass_session_id, "--pass-session-id");
        push_flag(args, self.ignore_user_config, "--ignore-user-config");
        push_flag(args, self.ignore_rules, "--ignore-rules");
        push_flag(args, self.safe_mode, "--safe-mode");
        push_opt(args, "--source", self.source.as_deref());
        push_flag(args, self.tui, "--tui");
        push_flag(args, self.cli, "--cli");
        push_flag(args, self.dev, "--dev");
    }
}

/// `hermes chat [OPTIONS]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatCommand {
    /// The single query for non-interactive mode (`-q`). When absent the chat
    /// session is interactive.
    pub query: Option<String>,
    /// Chat options.
    pub options: ChatOptions,
}

impl ChatCommand {
    /// `hermes chat` with no query — an interactive session.
    #[must_use]
    pub fn interactive() -> Self {
        Self::default()
    }

    /// `hermes chat -Q -q <QUERY>` — the quiet, single-query programmatic mode.
    #[must_use]
    pub fn quiet_query(query: impl Into<String>) -> Self {
        Self {
            query: Some(query.into()),
            options: ChatOptions {
                quiet: true,
                ..ChatOptions::default()
            },
        }
    }
}

impl ToArgs for ChatCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("chat".into());
        push_opt(args, "--query", self.query.as_deref());
        self.options.render(args);
    }
}
