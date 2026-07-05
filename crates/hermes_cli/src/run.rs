//! `hermes [OPTIONS]` — the root run in its four modes.
//!
//! With no subcommand, `hermes` runs the model. How the prompt is delivered and
//! which interface starts is selected by [`RunMode`]:
//!
//! * [`RunMode::OneShot`] — `-z <PROMPT>` (`--oneshot`): send a single prompt
//!   non-interactively and print ONLY the final response text to stdout. This
//!   is the scripted / pipe agent entry point; the prompt is the value of `-z`,
//!   so nothing is fed on stdin.
//! * [`RunMode::Tui`] — `--tui`: launch the modern TUI.
//! * [`RunMode::Cli`] — `--cli`: force the classic prompt_toolkit REPL.
//! * [`RunMode::Interactive`] — the default REPL.
//!
//! Hermes' root parser is Python's `argparse`, whose only positional is the
//! subcommand choice; the run modes above are all optional flags. Each
//! [`RunMode`] carries its own operand so mode and prompt can never desync:
//! [`OneShot`](RunMode::OneShot) *requires* its `-z` prompt, while the
//! interactive modes take an optional trailing-operand seed.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt, push_pair};

/// How the prompt is delivered and which interface the root `hermes` run
/// starts.
///
/// Each variant owns its operand, so a mode can never be paired with a prompt
/// it does not use (or, for [`OneShot`](Self::OneShot), be missing the prompt
/// it requires).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunMode {
    /// Default: launch the classic interactive REPL, optionally seeded by a
    /// trailing-operand prompt.
    Interactive(Option<String>),
    /// `-z` / `--oneshot`: run a single prompt non-interactively and print the
    /// final response text. The prompt is the required value of `-z`.
    OneShot(String),
    /// `--tui`: launch the modern TUI, optionally seeded by a trailing-operand
    /// prompt.
    Tui(Option<String>),
    /// `--cli`: force the classic prompt_toolkit REPL, optionally seeded by a
    /// trailing-operand prompt.
    Cli(Option<String>),
}

impl Default for RunMode {
    fn default() -> Self {
        Self::Interactive(None)
    }
}

/// How `--continue` is delivered. The flag takes an optional session name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContinueMode {
    /// `--continue` with no name: resume the most recent session.
    MostRecent,
    /// `--continue <NAME>`: resume a session by name.
    Named(String),
}

/// Root-run options shared across every [`RunMode`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `-m` / `--model <MODEL>`: model override for this invocation.
    pub model: Option<String>,
    /// `--provider <PROVIDER>`: provider override for this invocation.
    pub provider: Option<String>,
    /// `-t` / `--toolsets <TOOLSETS>`: comma-separated toolsets to enable.
    pub toolsets: Option<String>,
    /// `-s` / `--skills <SKILLS>`: preload one or more skills.
    pub skills: Option<String>,
    /// `-r` / `--resume <SESSION>`: resume a previous session by ID or title.
    pub resume: Option<String>,
    /// `-c` / `--continue [SESSION_NAME]`: resume a session by name, or the most
    /// recent when no name is given.
    pub continue_session: Option<ContinueMode>,
    /// `-w` / `--worktree`: run in an isolated git worktree.
    pub worktree: bool,
    /// `--accept-hooks`: auto-approve unseen shell hooks without a TTY prompt.
    pub accept_hooks: bool,
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
    /// `--dev`: with `--tui`, run the TypeScript sources via tsx.
    pub dev: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--provider", self.provider.as_deref());
        push_opt(args, "--toolsets", self.toolsets.as_deref());
        push_opt(args, "--skills", self.skills.as_deref());
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
        push_flag(args, self.yolo, "--yolo");
        push_flag(args, self.pass_session_id, "--pass-session-id");
        push_flag(args, self.ignore_user_config, "--ignore-user-config");
        push_flag(args, self.ignore_rules, "--ignore-rules");
        push_flag(args, self.safe_mode, "--safe-mode");
        push_flag(args, self.dev, "--dev");
    }
}

/// `hermes [OPTIONS]` — the root run.
///
/// The prompt (when any) rides inside [`mode`](Self::mode), so the interface
/// selected and the operand it carries can never disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// How the prompt is delivered and which interface starts.
    pub mode: RunMode,
    /// Root options rendered before the mode-specific part.
    pub options: RunOptions,
}

impl RunCommand {
    /// `-z <PROMPT>`: run a single prompt non-interactively.
    #[must_use]
    pub fn oneshot(prompt: impl Into<String>) -> Self {
        Self {
            mode: RunMode::OneShot(prompt.into()),
            ..Self::default()
        }
    }

    /// `--tui`: launch the modern TUI with no seed prompt.
    #[must_use]
    pub fn tui() -> Self {
        Self {
            mode: RunMode::Tui(None),
            ..Self::default()
        }
    }

    /// `--tui <PROMPT>`: launch the modern TUI seeded by a prompt.
    #[must_use]
    pub fn tui_prompt(prompt: impl Into<String>) -> Self {
        Self {
            mode: RunMode::Tui(Some(prompt.into())),
            ..Self::default()
        }
    }

    /// The default interactive REPL with no seed prompt.
    #[must_use]
    pub fn interactive() -> Self {
        Self {
            mode: RunMode::Interactive(None),
            ..Self::default()
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.options.render(args);
        match &self.mode {
            RunMode::OneShot(prompt) => push_pair(args, "-z", prompt),
            RunMode::Tui(seed) => {
                args.push("--tui".into());
                if let Some(seed) = seed {
                    args.push(seed.into());
                }
            }
            RunMode::Cli(seed) => {
                args.push("--cli".into());
                if let Some(seed) = seed {
                    args.push(seed.into());
                }
            }
            RunMode::Interactive(seed) => {
                if let Some(seed) = seed {
                    args.push(seed.into());
                }
            }
        }
    }
}
