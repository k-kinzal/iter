//! `grok [OPTIONS] [PROMPT]` — the root interactive run, and the shared
//! behavioral options it and the headless `single` run both accept.
//!
//! With no subcommand, Grok launches its TUI seeded by an optional prompt
//! positional. The headless single-turn run (`grok -p <PROMPT>`, see
//! [`single`](crate::single)) shares the same flag surface, so the behavioral
//! options live in [`RunOptions`] and both command builders embed it.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_each, push_enum, push_flag, push_opt, push_opt_num, push_opt_path};
use crate::options::GlobalOptions;
use crate::values::{Effort, PermissionMode, ResumeTarget, Worktree};

/// Behavioral options shared by `grok [PROMPT]` and `grok -p <PROMPT>`.
///
/// These render in a stable order so argv snapshots are deterministic. The
/// prompt-delivery flags (`-p`/`--prompt-file`/`--prompt-json`) and
/// `--output-format` are modeled on the [`SingleCommand`](crate::SingleCommand)
/// builder instead, since they select *how* a headless run is driven.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// `--agent <NAME>`: agent name or definition file path.
    pub agent: Option<String>,
    /// `--agents <JSON>`: inline subagent definitions as JSON.
    pub agents: Option<String>,
    /// `--allow <RULE>` (repeatable): permission allow rule.
    pub allow: Vec<String>,
    /// `--deny <RULE>` (repeatable): permission deny rule.
    pub deny: Vec<String>,
    /// `--always-approve`: auto-approve all tool executions.
    pub always_approve: bool,
    /// `--best-of-n <N>`: run the task N ways in parallel (headless only).
    pub best_of_n: Option<u64>,
    /// `-c, --continue`: continue the most recent session for the cwd.
    pub continue_session: bool,
    /// `--chat`: open the session as a gateway light-frontend (`kind: chat`).
    pub chat: bool,
    /// `--check`: append a self-verification loop to the prompt (headless only).
    pub check: bool,
    /// `--cwd <CWD>`: working directory.
    pub cwd: Option<PathBuf>,
    /// `--disable-web-search`: disable web search and web fetch tools.
    pub disable_web_search: bool,
    /// `--disallowed-tools <TOOLS>`: built-in tools to remove (comma-separated).
    pub disallowed_tools: Option<String>,
    /// `--effort <LEVEL>`.
    pub effort: Option<Effort>,
    /// `--experimental-memory`: enable cross-session memory.
    pub experimental_memory: bool,
    /// `--fork-session`: on resume, mint a new session ID instead of reusing it.
    pub fork_session: bool,
    /// `--json-schema <SCHEMA>`: constrain output to this JSON Schema (implies
    /// `--output-format json`).
    pub json_schema: Option<String>,
    /// `-m, --model <MODEL>`.
    pub model: Option<String>,
    /// `--max-turns <N>`: maximum number of agent turns.
    pub max_turns: Option<u64>,
    /// `--no-alt-screen`: run inline instead of the alternate screen.
    pub no_alt_screen: bool,
    /// `--no-memory`: disable cross-session memory for this session.
    pub no_memory: bool,
    /// `--no-plan`: disable plan mode.
    pub no_plan: bool,
    /// `--no-subagents`: disable subagent spawning.
    pub no_subagents: bool,
    /// `--oauth`: use OAuth when the welcome screen starts authentication.
    pub oauth: bool,
    /// `--permission-mode <MODE>`.
    pub permission_mode: Option<PermissionMode>,
    /// `-r, --resume [<SESSION_ID>]`.
    pub resume: Option<ResumeTarget>,
    /// `--reasoning-effort <EFFORT>`: reasoning effort for reasoning models.
    pub reasoning_effort: Option<String>,
    /// `--restore-code`: check out the original session's commit on resume.
    pub restore_code: bool,
    /// `--rules <RULES>`: extra rules to append to the system prompt.
    pub rules: Option<String>,
    /// `-s, --session-id <SESSION_ID>`: session UUID for a new conversation.
    pub session_id: Option<String>,
    /// `--sandbox <PROFILE>`: sandbox profile for filesystem/network access.
    pub sandbox: Option<String>,
    /// `--system-prompt-override <PROMPT>`.
    pub system_prompt_override: Option<String>,
    /// `--tools <TOOLS>`: built-in tools to allow (comma-separated).
    pub tools: Option<String>,
    /// `--verbatim`: send the prompt exactly as given.
    pub verbatim: bool,
    /// `-w, --worktree [<WORKTREE>]`.
    pub worktree: Option<Worktree>,
    /// `--worktree-ref <WORKTREE_REF>`: branch/tag/commit to base the worktree
    /// on (with `--worktree`).
    pub worktree_ref: Option<String>,
}

impl RunOptions {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        self.global.render(args);
        push_opt(args, "--agent", self.agent.as_deref());
        push_opt(args, "--agents", self.agents.as_deref());
        push_each(args, "--allow", &self.allow);
        push_each(args, "--deny", &self.deny);
        push_flag(args, self.always_approve, "--always-approve");
        push_opt_num(args, "--best-of-n", self.best_of_n);
        push_flag(args, self.continue_session, "--continue");
        push_flag(args, self.chat, "--chat");
        push_flag(args, self.check, "--check");
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_flag(args, self.disable_web_search, "--disable-web-search");
        push_opt(args, "--disallowed-tools", self.disallowed_tools.as_deref());
        push_enum(args, "--effort", self.effort.map(Effort::as_str));
        push_flag(args, self.experimental_memory, "--experimental-memory");
        push_flag(args, self.fork_session, "--fork-session");
        push_opt(args, "--json-schema", self.json_schema.as_deref());
        push_opt(args, "--model", self.model.as_deref());
        push_opt_num(args, "--max-turns", self.max_turns);
        push_flag(args, self.no_alt_screen, "--no-alt-screen");
        push_flag(args, self.no_memory, "--no-memory");
        push_flag(args, self.no_plan, "--no-plan");
        push_flag(args, self.no_subagents, "--no-subagents");
        push_flag(args, self.oauth, "--oauth");
        push_enum(
            args,
            "--permission-mode",
            self.permission_mode.map(PermissionMode::as_str),
        );
        match &self.resume {
            // `-r [<SESSION_ID>]` — with an id, resume that session; without,
            // the most recent one.
            Some(ResumeTarget::Session(id)) => {
                args.push("-r".into());
                args.push(id.into());
            }
            Some(ResumeTarget::MostRecent) => args.push("-r".into()),
            None => {}
        }
        push_opt(args, "--reasoning-effort", self.reasoning_effort.as_deref());
        push_flag(args, self.restore_code, "--restore-code");
        push_opt(args, "--rules", self.rules.as_deref());
        push_opt(args, "--session-id", self.session_id.as_deref());
        push_opt(args, "--sandbox", self.sandbox.as_deref());
        push_opt(
            args,
            "--system-prompt-override",
            self.system_prompt_override.as_deref(),
        );
        push_opt(args, "--tools", self.tools.as_deref());
        push_flag(args, self.verbatim, "--verbatim");
        match &self.worktree {
            Some(Worktree::Named(name)) => {
                args.push("-w".into());
                args.push(name.into());
            }
            Some(Worktree::Auto) => args.push("-w".into()),
            None => {}
        }
        push_opt(args, "--worktree-ref", self.worktree_ref.as_deref());
    }
}

/// `grok [OPTIONS] [PROMPT]` — the root interactive TUI run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// Behavioral options shared with the headless run.
    pub options: RunOptions,
    /// Optional prompt positional seeding the interactive session.
    pub prompt: Option<String>,
}

impl RunCommand {
    /// Build a root run seeded with a prompt positional.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.options.render(args);
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}
