//! The root `cursor-agent` run and its `--print` headless mode.
//!
//! `cursor-agent [OPTIONS] [PROMPT...]` starts the Cursor Agent. With no
//! `--print` it launches the interactive TUI; with `--print` it runs headless
//! and emits machine-readable output selected by `--output-format`. The full
//! option set (model selection, approval/sandbox, workspace/worktree, session
//! continuity) is shared by both and lives in [`RunOptions`].
//!
//! The explicit `agent` subcommand is the same entry point reached by name;
//! it is modeled by [`AgentCommand`].

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_enum, push_flag, push_opt, push_opt_path, push_pair};
use crate::values::{ExecutionMode, OutputFormat, ResumeSelector, SandboxMode, Worktree};

/// Options accepted by the root `cursor-agent` run.
///
/// These render in a stable order so argv snapshots are deterministic. Options
/// that only take effect with `--print` (the output format and partial-output
/// streaming) live on [`PrintCommand`] instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `--api-key <key>` (otherwise read from `CURSOR_API_KEY`).
    pub api_key: Option<String>,
    /// `-H, --header <header>` (repeatable): extra headers, each a
    /// `(name, value)` pair rendered as `Name: Value`.
    pub headers: Vec<(String, String)>,
    /// `-c, --cloud`: start in cloud mode (open the composer picker on launch).
    pub cloud: bool,
    /// `--mode <mode>`: start in a read-only execution mode.
    pub mode: Option<ExecutionMode>,
    /// `--plan`: shorthand for `--mode=plan`.
    pub plan: bool,
    /// `--resume [chatId]`: resume an existing chat session.
    pub resume: Option<ResumeSelector>,
    /// `--continue`: continue the previous session.
    pub continue_session: bool,
    /// `--model <model>` (e.g. `gpt-5`, `sonnet-4`, `sonnet-4-thinking`).
    pub model: Option<String>,
    /// `--list-models`: list available models and exit.
    pub list_models: bool,
    /// `-f, --force`: force-allow commands unless explicitly denied.
    pub force: bool,
    /// `--yolo`: alias for `--force` (run everything).
    pub yolo: bool,
    /// `--sandbox <mode>`: explicitly enable or disable sandbox mode.
    pub sandbox: Option<SandboxMode>,
    /// `--approve-mcps`: automatically approve all MCP servers.
    pub approve_mcps: bool,
    /// `--trust`: trust the current workspace without prompting
    /// (only with `--print`/headless mode).
    pub trust: bool,
    /// `--workspace <path>`: workspace directory to use.
    pub workspace: Option<PathBuf>,
    /// `-w, --worktree [name]`: run in an isolated git worktree.
    pub worktree: Option<Worktree>,
    /// `--worktree-base <branch>`: branch or ref to base the new worktree on.
    pub worktree_base: Option<String>,
    /// `--skip-worktree-setup`: skip running worktree setup scripts.
    pub skip_worktree_setup: bool,
}

impl RunOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--api-key", self.api_key.as_deref());
        render_headers(args, &self.headers);
        push_flag(args, self.cloud, "--cloud");
        push_enum(args, "--mode", self.mode.map(ExecutionMode::as_str));
        push_flag(args, self.plan, "--plan");
        render_resume(args, self.resume.as_ref());
        push_flag(args, self.continue_session, "--continue");
        push_opt(args, "--model", self.model.as_deref());
        push_flag(args, self.list_models, "--list-models");
        push_flag(args, self.force, "--force");
        push_flag(args, self.yolo, "--yolo");
        push_enum(args, "--sandbox", self.sandbox.map(SandboxMode::as_str));
        push_flag(args, self.approve_mcps, "--approve-mcps");
        push_flag(args, self.trust, "--trust");
        push_opt_path(args, "--workspace", self.workspace.as_deref());
        render_worktree(args, self.worktree.as_ref());
        push_opt(args, "--worktree-base", self.worktree_base.as_deref());
        push_flag(args, self.skip_worktree_setup, "--skip-worktree-setup");
    }
}

/// Render the repeatable `-H, --header` flag, joining each `(name, value)`
/// pair with the CLI's `Name: Value` separator.
fn render_headers(args: &mut Vec<OsString>, headers: &[(String, String)]) {
    for (name, value) in headers {
        push_pair(args, "--header", format!("{name}: {value}"));
    }
}

/// Render `--resume`, respecting its optional-value shape.
fn render_resume(args: &mut Vec<OsString>, resume: Option<&ResumeSelector>) {
    match resume {
        None => {}
        Some(ResumeSelector::Prompt) => args.push("--resume".into()),
        Some(ResumeSelector::Chat(id)) => {
            args.push("--resume".into());
            args.push(id.into());
        }
    }
}

/// Render `--worktree`, respecting its optional-value shape.
fn render_worktree(args: &mut Vec<OsString>, worktree: Option<&Worktree>) {
    match worktree {
        None => {}
        Some(Worktree::Auto) => args.push("--worktree".into()),
        Some(Worktree::Named(name)) => {
            args.push("--worktree".into());
            args.push(name.into());
        }
    }
}

/// Render the variadic `[prompt...]` positional.
fn render_prompt(args: &mut Vec<OsString>, prompt: &[String]) {
    for word in prompt {
        args.push(word.into());
    }
}

/// `cursor-agent [OPTIONS] [PROMPT...]` — the interactive root run.
///
/// Without `--print` the CLI launches its TUI seeded by the optional prompt.
/// For the headless, machine-readable mode use [`PrintCommand`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// The shared agent options.
    pub options: RunOptions,
    /// The variadic prompt positional seeding the first turn.
    pub prompt: Vec<String>,
}

impl RunCommand {
    /// Build a root run seeded with a single prompt string.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: vec![prompt.into()],
            ..Self::default()
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.options.render(args);
        render_prompt(args, &self.prompt);
    }
}

/// `cursor-agent --print [OPTIONS] [PROMPT...]` — the headless run.
///
/// `--print` makes `cursor-agent` emit a single response and exit;
/// `--output-format` selects `text`, `json`, or `stream-json`. Executing a
/// `json`/`stream-json` command through [`Cursor::execute`](crate::Cursor::execute)
/// yields a typed [`PrintOutput`](crate::PrintOutput).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintCommand {
    /// The shared agent options.
    pub options: RunOptions,
    /// `--output-format <format>`.
    pub output_format: OutputFormat,
    /// `--stream-partial-output`: stream partial output as individual text
    /// deltas (only with `--print` and `stream-json`).
    pub stream_partial_output: bool,
    /// The variadic prompt positional.
    pub prompt: Vec<String>,
}

impl PrintCommand {
    fn with_format(output_format: OutputFormat) -> Self {
        Self {
            options: RunOptions::default(),
            output_format,
            stream_partial_output: false,
            prompt: Vec::new(),
        }
    }

    /// Build a `--print --output-format json` run.
    #[must_use]
    pub fn json() -> Self {
        Self::with_format(OutputFormat::Json)
    }

    /// Build a `--print --output-format stream-json` run.
    #[must_use]
    pub fn stream_json() -> Self {
        Self::with_format(OutputFormat::StreamJson)
    }

    /// Build a `--print --output-format text` run.
    #[must_use]
    pub fn text() -> Self {
        Self::with_format(OutputFormat::Text)
    }

    /// Append a prompt word to the variadic positional.
    #[must_use]
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt.push(prompt.into());
        self
    }
}

impl ToArgs for PrintCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("--print".into());
        args.push("--output-format".into());
        args.push(self.output_format.as_str().into());
        push_flag(args, self.stream_partial_output, "--stream-partial-output");
        self.options.render(args);
        render_prompt(args, &self.prompt);
    }
}

/// `cursor-agent agent [PROMPT...]` — the explicit agent subcommand.
///
/// The default (no-subcommand) invocation already starts the agent; this is
/// the same entry point reached by naming `agent` explicitly. The CLI exposes
/// only the prompt positional on this subcommand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCommand {
    /// The variadic prompt positional.
    pub prompt: Vec<String>,
}

impl AgentCommand {
    /// Build an `agent` run seeded with a single prompt string.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: vec![prompt.into()],
        }
    }
}

impl ToArgs for AgentCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("agent".into());
        render_prompt(args, &self.prompt);
    }
}
