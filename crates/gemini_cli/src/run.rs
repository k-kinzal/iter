//! `gemini [query..]` — the root run command.
//!
//! Gemini defaults to an interactive TUI seeded by an optional `[query..]`
//! positional. Passing `-p/--prompt` switches to non-interactive (headless)
//! mode; `-o/--output-format json|stream-json` then makes the run's output
//! machine-readable. The typed-output wrappers [`JsonRunCommand`] and
//! [`StreamRunCommand`] append the `-o` selector and pin the parser.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    ToArgs, push_each, push_enum, push_flag, push_num, push_opt, push_pair, push_paths,
};
use crate::values::{ApprovalMode, OutputFormat, SessionRef, Worktree};

/// Options for the root `gemini` run (everything but the `-p` prompt and the
/// `[query..]` positional).
///
/// These render in a stable order so argv snapshots are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `-d, --debug`.
    pub debug: bool,
    /// `-m, --model <MODEL>`.
    pub model: Option<String>,
    /// `-i, --prompt-interactive <PROMPT>`: run the prompt then stay
    /// interactive.
    pub prompt_interactive: Option<String>,
    /// `--skip-trust`: trust the current workspace for this session.
    pub skip_trust: bool,
    /// `-w, --worktree [NAME]`.
    pub worktree: Option<Worktree>,
    /// `-s, --sandbox`.
    pub sandbox: bool,
    /// `-y, --yolo`: auto-approve all actions.
    pub yolo: bool,
    /// `--approval-mode <MODE>`.
    pub approval_mode: Option<ApprovalMode>,
    /// `--policy <PATH>` (repeatable).
    pub policy: Vec<PathBuf>,
    /// `--admin-policy <PATH>` (repeatable).
    pub admin_policy: Vec<PathBuf>,
    /// `--acp`: start the agent in ACP mode.
    pub acp: bool,
    /// `--experimental-acp`: deprecated alias for `--acp`.
    pub experimental_acp: bool,
    /// `--allowed-mcp-server-names <NAME>` (repeatable).
    pub allowed_mcp_server_names: Vec<String>,
    /// `--allowed-tools <TOOL>` (repeatable; deprecated by Gemini).
    pub allowed_tools: Vec<String>,
    /// `-e, --extensions <NAME>` (repeatable).
    pub extensions: Vec<String>,
    /// `-l, --list-extensions`: list extensions and exit.
    pub list_extensions: bool,
    /// `-r, --resume <SESSION>`: resume a previous session (`latest` or index).
    pub resume: Option<SessionRef>,
    /// `--session-id <UUID>`: start a new session with a fixed UUID.
    pub session_id: Option<String>,
    /// `--list-sessions`: list sessions for the project and exit.
    pub list_sessions: bool,
    /// `--delete-session <INDEX>`: delete a session by index number.
    pub delete_session: Option<u64>,
    /// `--include-directories <DIR>` (repeatable).
    pub include_directories: Vec<PathBuf>,
    /// `--screen-reader`: enable screen-reader accessibility mode.
    pub screen_reader: bool,
    /// `--raw-output`: disable sanitization of model output.
    pub raw_output: bool,
    /// `--accept-raw-output-risk`: suppress the `--raw-output` warning.
    pub accept_raw_output_risk: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.debug, "--debug");
        push_opt(args, "--model", self.model.as_deref());
        push_opt(
            args,
            "--prompt-interactive",
            self.prompt_interactive.as_deref(),
        );
        push_flag(args, self.skip_trust, "--skip-trust");
        match &self.worktree {
            Some(Worktree::Auto) => args.push("--worktree".into()),
            Some(Worktree::Named(name)) => {
                args.push("--worktree".into());
                args.push(name.into());
            }
            None => {}
        }
        push_flag(args, self.sandbox, "--sandbox");
        push_flag(args, self.yolo, "--yolo");
        push_enum(
            args,
            "--approval-mode",
            self.approval_mode.map(ApprovalMode::as_str),
        );
        push_paths(args, "--policy", &self.policy);
        push_paths(args, "--admin-policy", &self.admin_policy);
        push_flag(args, self.acp, "--acp");
        push_flag(args, self.experimental_acp, "--experimental-acp");
        push_each(
            args,
            "--allowed-mcp-server-names",
            &self.allowed_mcp_server_names,
        );
        push_each(args, "--allowed-tools", &self.allowed_tools);
        push_each(args, "--extensions", &self.extensions);
        push_flag(args, self.list_extensions, "--list-extensions");
        if let Some(resume) = &self.resume {
            push_pair(args, "--resume", resume.as_arg());
        }
        push_opt(args, "--session-id", self.session_id.as_deref());
        push_flag(args, self.list_sessions, "--list-sessions");
        push_num(args, "--delete-session", self.delete_session);
        push_paths(args, "--include-directories", &self.include_directories);
        push_flag(args, self.screen_reader, "--screen-reader");
        push_flag(args, self.raw_output, "--raw-output");
        push_flag(
            args,
            self.accept_raw_output_risk,
            "--accept-raw-output-risk",
        );
    }
}

/// `gemini [OPTIONS] [-p PROMPT] [query..]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// Root-run options.
    pub options: RunOptions,
    /// `-p, --prompt <PROMPT>`: the non-interactive (headless) prompt.
    pub prompt: Option<String>,
    /// `[query..]` positional(s): the interactive-mode initial prompt.
    pub query: Vec<String>,
}

impl RunCommand {
    /// Build a headless run with a `-p/--prompt` value.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }

    /// Build an interactive run seeded with a `[query..]` positional.
    #[must_use]
    pub fn query(query: impl Into<String>) -> Self {
        Self {
            query: vec![query.into()],
            ..Self::default()
        }
    }

    /// Select `gemini -o json`, yielding a single typed
    /// [`GeminiOutput`](crate::GeminiOutput) terminal record.
    #[must_use]
    pub fn json(self) -> JsonRunCommand {
        JsonRunCommand { command: self }
    }

    /// Select `gemini -o stream-json`, yielding a newline-delimited event
    /// stream consumed via [`Gemini::stream`](crate::Gemini::stream).
    #[must_use]
    pub fn stream_json(self) -> StreamRunCommand {
        StreamRunCommand { command: self }
    }

    fn render(&self, args: &mut Vec<OsString>, format: Option<OutputFormat>) {
        self.options.render(args);
        push_enum(args, "--output-format", format.map(OutputFormat::as_str));
        push_opt(args, "--prompt", self.prompt.as_deref());
        for query in &self.query {
            args.push(query.into());
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args, None);
    }
}

/// `gemini -o json [OPTIONS] [-p PROMPT] [query..]`.
///
/// [`Gemini::execute`](crate::Gemini::execute) returns the single terminal
/// [`GeminiOutput`](crate::GeminiOutput) record.
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
        self.command.render(args, Some(OutputFormat::Json));
    }
}

/// `gemini -o stream-json [OPTIONS] [-p PROMPT] [query..]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRunCommand {
    command: RunCommand,
}

impl StreamRunCommand {
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

impl ToArgs for StreamRunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.render(args, Some(OutputFormat::StreamJson));
    }
}
