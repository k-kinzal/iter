//! `codex exec` — the non-interactive run command and its subcommands.
//!
//! `codex exec [OPTIONS] [PROMPT]` is Codex's one-shot headless mode. With
//! `--json` it streams the JSONL event stream [`ExecOutput`](crate::ExecOutput)
//! parses. The `exec resume` and `exec review` subcommands are modeled as
//! their own builders.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_each, push_enum, push_flag, push_opt, push_opt_path, push_paths};
use crate::options::CommonConfig;
use crate::values::{Color, ConfigOverride};

/// Options specific to `codex exec` (beyond the shared [`CommonConfig`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOptions {
    /// `--skip-git-repo-check`.
    pub skip_git_repo_check: bool,
    /// `--ephemeral`.
    pub ephemeral: bool,
    /// `--ignore-user-config`.
    pub ignore_user_config: bool,
    /// `--ignore-rules`.
    pub ignore_rules: bool,
    /// `--output-schema <FILE>`.
    pub output_schema: Option<PathBuf>,
    /// `--color <COLOR>`.
    pub color: Option<Color>,
    /// `-o, --output-last-message <FILE>`.
    pub output_last_message: Option<PathBuf>,
}

impl ExecOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.skip_git_repo_check, "--skip-git-repo-check");
        push_flag(args, self.ephemeral, "--ephemeral");
        push_flag(args, self.ignore_user_config, "--ignore-user-config");
        push_flag(args, self.ignore_rules, "--ignore-rules");
        push_opt_path(args, "--output-schema", self.output_schema.as_deref());
        push_enum(args, "--color", self.color.map(Color::as_str));
        push_opt_path(
            args,
            "--output-last-message",
            self.output_last_message.as_deref(),
        );
    }
}

/// `codex exec [OPTIONS] [PROMPT]`.
///
/// The plain form renders without `--json`. Call [`ExecCommand::json`] to
/// request the JSONL event stream and a typed [`ExecOutput`](crate::ExecOutput).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecCommand {
    /// Options common to the root run and `exec`.
    pub common: CommonConfig,
    /// `exec`-specific options.
    pub options: ExecOptions,
    /// Optional prompt positional. `None`/`-` reads instructions from stdin.
    pub prompt: Option<String>,
}

impl ExecCommand {
    /// Build an `exec` run with a prompt positional argument.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }

    /// Select `codex exec --json`, yielding a typed
    /// [`ExecOutput`](crate::ExecOutput).
    #[must_use]
    pub fn json(self) -> JsonExecCommand {
        JsonExecCommand { command: self }
    }

    fn render(&self, args: &mut Vec<OsString>, json: bool) {
        args.push("exec".into());
        push_flag(args, json, "--json");
        self.common.render(args);
        self.options.render(args);
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}

impl ToArgs for ExecCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args, false);
    }
}

/// `codex exec --json [OPTIONS] [PROMPT]`.
///
/// [`Codex::execute`](crate::Codex::execute) returns
/// [`ExecOutput`](crate::ExecOutput).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonExecCommand {
    command: ExecCommand,
}

impl JsonExecCommand {
    /// Borrow the underlying `exec` configuration.
    #[must_use]
    pub const fn command(&self) -> &ExecCommand {
        &self.command
    }

    /// Return the underlying `exec` configuration.
    #[must_use]
    pub fn into_command(self) -> ExecCommand {
        self.command
    }
}

impl ToArgs for JsonExecCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.render(args, true);
    }
}

/// Options shared by `codex exec resume` and `codex exec review`.
///
/// This is a **narrowed** subset of the base [`ExecCommand`] options: both
/// subcommands' `--help` omit `--oss`, `--local-provider`, `-p/--profile`,
/// `-s/--sandbox`, `-C/--cd`, `--add-dir`, and `--color`, so those are not
/// modeled here and cannot be emitted. (`exec resume` adds `-i/--image`, which
/// `exec review` does not accept; it lives on [`ExecResumeCommand`] instead.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecSubcommandOptions {
    /// `-c, --config key=value` (repeatable).
    pub config: Vec<ConfigOverride>,
    /// `--enable <FEATURE>` (repeatable).
    pub enable: Vec<String>,
    /// `--disable <FEATURE>` (repeatable).
    pub disable: Vec<String>,
    /// `--strict-config`.
    pub strict_config: bool,
    /// `-m, --model <MODEL>`.
    pub model: Option<String>,
    /// `--dangerously-bypass-approvals-and-sandbox`.
    pub dangerously_bypass_approvals_and_sandbox: bool,
    /// `--dangerously-bypass-hook-trust`.
    pub dangerously_bypass_hook_trust: bool,
    /// `--skip-git-repo-check`.
    pub skip_git_repo_check: bool,
    /// `--ephemeral`.
    pub ephemeral: bool,
    /// `--ignore-user-config`.
    pub ignore_user_config: bool,
    /// `--ignore-rules`.
    pub ignore_rules: bool,
    /// `--output-schema <FILE>`.
    pub output_schema: Option<PathBuf>,
    /// `-o, --output-last-message <FILE>`.
    pub output_last_message: Option<PathBuf>,
}

impl ExecSubcommandOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        for override_ in &self.config {
            args.push("--config".into());
            args.push(override_.render().into());
        }
        push_each(args, "--enable", &self.enable);
        push_each(args, "--disable", &self.disable);
        push_flag(args, self.strict_config, "--strict-config");
        push_opt(args, "--model", self.model.as_deref());
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
        push_flag(args, self.skip_git_repo_check, "--skip-git-repo-check");
        push_flag(args, self.ephemeral, "--ephemeral");
        push_flag(args, self.ignore_user_config, "--ignore-user-config");
        push_flag(args, self.ignore_rules, "--ignore-rules");
        push_opt_path(args, "--output-schema", self.output_schema.as_deref());
        push_opt_path(
            args,
            "--output-last-message",
            self.output_last_message.as_deref(),
        );
    }
}

/// `codex exec resume [OPTIONS] [SESSION_ID] [PROMPT]`.
///
/// A narrowed sibling of [`ExecCommand`]: it carries [`ExecSubcommandOptions`]
/// plus resume-specific `--last`/`--all` and the repeatable `-i/--image`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecResumeCommand {
    /// Narrowed config/model/execution options.
    pub options: ExecSubcommandOptions,
    /// `-i, --image <FILE>` (repeatable).
    pub images: Vec<PathBuf>,
    /// `--last`: continue the most recent session without a picker.
    pub last: bool,
    /// `--all`: show all sessions (disables cwd filtering).
    pub all: bool,
    /// `--json`.
    pub json: bool,
    /// Optional `[SESSION_ID]` positional.
    pub session_id: Option<String>,
    /// Optional `[PROMPT]` positional.
    pub prompt: Option<String>,
}

impl ToArgs for ExecResumeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("exec".into());
        args.push("resume".into());
        push_flag(args, self.json, "--json");
        push_flag(args, self.last, "--last");
        push_flag(args, self.all, "--all");
        push_paths(args, "--image", &self.images);
        self.options.render(args);
        if let Some(session_id) = &self.session_id {
            args.push(session_id.into());
        }
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}

/// `codex exec review [OPTIONS] [PROMPT]`.
///
/// A narrowed sibling of [`ExecCommand`]: it carries [`ExecSubcommandOptions`]
/// plus the review selectors (`--uncommitted`, `--base`, `--commit`,
/// `--title`). It does not accept `-i/--image`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecReviewCommand {
    /// Narrowed config/model/execution options.
    pub options: ExecSubcommandOptions,
    /// `--json`.
    pub json: bool,
    /// `--uncommitted`: review staged, unstaged, and untracked changes.
    pub uncommitted: bool,
    /// `--base <BRANCH>`: review changes against the given base branch.
    pub base: Option<String>,
    /// `--commit <SHA>`: review the changes introduced by a commit.
    pub commit: Option<String>,
    /// `--title <TITLE>`: title displayed in the review summary.
    pub title: Option<String>,
    /// Optional `[PROMPT]` positional (custom review instructions).
    pub prompt: Option<String>,
}

impl ToArgs for ExecReviewCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("exec".into());
        args.push("review".into());
        push_flag(args, self.json, "--json");
        push_flag(args, self.uncommitted, "--uncommitted");
        push_opt(args, "--base", self.base.as_deref());
        push_opt(args, "--commit", self.commit.as_deref());
        push_opt(args, "--title", self.title.as_deref());
        self.options.render(args);
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}
