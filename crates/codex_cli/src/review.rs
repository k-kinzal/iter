//! `codex review [OPTIONS] [PROMPT]` — the code-review command.
//!
//! Unlike the root run and `exec`, `codex review --help` accepts only a narrow
//! set of flags: the config family (`-c/--config`, `--enable`, `--disable`,
//! `--strict-config`) plus the review selectors (`--uncommitted`, `--base`,
//! `--commit`, `--title`). It does **not** accept the run/model family
//! (`--image`, `--model`, `--oss`, `--sandbox`, `--cd`, …), so this builder
//! deliberately cannot emit those.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt};
use crate::options::GlobalConfig;

/// `codex review [OPTIONS] [PROMPT]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewCommand {
    /// Shared `-c`/`--enable`/`--disable` options.
    pub global: GlobalConfig,
    /// `--strict-config`.
    pub strict_config: bool,
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

impl ToArgs for ReviewCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("review".into());
        self.global.render(args);
        push_flag(args, self.strict_config, "--strict-config");
        push_flag(args, self.uncommitted, "--uncommitted");
        push_opt(args, "--base", self.base.as_deref());
        push_opt(args, "--commit", self.commit.as_deref());
        push_opt(args, "--title", self.title.as_deref());
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}
