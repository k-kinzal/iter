//! `opencode run` — the non-interactive run command, and `opencode [project]`,
//! the root interactive TUI.
//!
//! `opencode run [OPTIONS] [message..]` is opencode's headless mode. With
//! `--format json` it emits the event stream [`RunOutput`](crate::RunOutput)
//! parses. The root command (`opencode [project]`) launches the interactive
//! TUI, optionally seeded by `--prompt`.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_enum, push_flag, push_opt, push_opt_display, push_paths};
use crate::options::{GlobalOptions, ServerOptions};
use crate::values::{Continuation, OutputFormat};

/// Options for `opencode run` beyond the shared [`GlobalOptions`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `--command <COMMAND>`: run a named command, using `message` as its args.
    pub command: Option<String>,
    /// Session continuation: `--continue` / `--session <id>` and the
    /// selector-gated `--fork`. Defaults to [`Continuation::Fresh`].
    pub continuation: Continuation,
    /// `--share`: share the session.
    pub share: bool,
    /// `-m, --model <PROVIDER/MODEL>`.
    pub model: Option<String>,
    /// `--agent <AGENT>`.
    pub agent: Option<String>,
    /// `--format <FORMAT>`. `None` leaves opencode's `default` format in place.
    pub format: Option<OutputFormat>,
    /// `-f, --file <FILE>` (repeatable): files to attach to the message.
    pub files: Vec<PathBuf>,
    /// `--title <TITLE>`.
    pub title: Option<String>,
    /// `--attach <URL>`: attach to a running opencode server.
    pub attach: Option<String>,
    /// `--dir <DIR>`: directory to run in (or path on the remote when attaching).
    pub dir: Option<String>,
    /// `--port <PORT>`: port for the local server.
    pub port: Option<u16>,
    /// `--variant <VARIANT>`: provider-specific reasoning effort.
    pub variant: Option<String>,
    /// `--thinking`: show thinking blocks.
    pub thinking: bool,
}

impl RunOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--command", self.command.as_deref());
        self.continuation.render(args);
        push_flag(args, self.share, "--share");
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--agent", self.agent.as_deref());
        push_enum(args, "--format", self.format.map(OutputFormat::as_str));
        push_paths(args, "--file", &self.files);
        push_opt(args, "--title", self.title.as_deref());
        push_opt(args, "--attach", self.attach.as_deref());
        push_opt(args, "--dir", self.dir.as_deref());
        push_opt_display(args, "--port", self.port);
        push_opt(args, "--variant", self.variant.as_deref());
        push_flag(args, self.thinking, "--thinking");
    }
}

/// `opencode run [OPTIONS] [message..]`.
///
/// The plain form renders whatever `--format` is set (opencode defaults to
/// `default`). Call [`RunCommand::json`] to force `--format json` and obtain a
/// typed [`RunOutput`](crate::RunOutput).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `run`-specific options.
    pub options: RunOptions,
    /// The `[message..]` positional(s). opencode reads stdin when empty.
    pub message: Vec<String>,
}

impl RunCommand {
    /// Build a `run` with a single message positional.
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: vec![message.into()],
            ..Self::default()
        }
    }

    /// Force `--format json`, yielding a typed [`RunOutput`](crate::RunOutput).
    #[must_use]
    pub fn json(mut self) -> JsonRunCommand {
        self.options.format = Some(OutputFormat::Json);
        JsonRunCommand { command: self }
    }

    fn render(&self, args: &mut Vec<OsString>) {
        args.push("run".into());
        self.global.render(args);
        self.options.render(args);
        for message in &self.message {
            args.push(message.into());
        }
    }
}

impl ToArgs for RunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args);
    }
}

/// `opencode run --format json [OPTIONS] [message..]`.
///
/// [`Opencode::execute`](crate::Opencode::execute) returns
/// [`RunOutput`](crate::RunOutput).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonRunCommand {
    command: RunCommand,
}

impl JsonRunCommand {
    /// Borrow the underlying `run` configuration.
    #[must_use]
    pub const fn command(&self) -> &RunCommand {
        &self.command
    }

    /// Return the underlying `run` configuration.
    #[must_use]
    pub fn into_command(self) -> RunCommand {
        self.command
    }
}

impl ToArgs for JsonRunCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.render(args);
    }
}

/// `opencode [OPTIONS] [project]` — the root interactive TUI.
///
/// With no subcommand, opencode launches its TUI in the optional `[project]`
/// directory, optionally seeded by `--prompt`. The root command carries the
/// server-hosting options (it can host a local server) plus session-selection
/// and model options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuiCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// Server-hosting options (`--port`, `--hostname`, mDNS, `--cors`).
    pub server: ServerOptions,
    /// `-m, --model <PROVIDER/MODEL>`.
    pub model: Option<String>,
    /// Session continuation: `--continue` / `--session <id>` and the
    /// selector-gated `--fork`. Defaults to [`Continuation::Fresh`].
    pub continuation: Continuation,
    /// `--prompt <PROMPT>`: seed the first turn.
    pub prompt: Option<String>,
    /// `--agent <AGENT>`.
    pub agent: Option<String>,
    /// Optional `[project]` positional: the path to start opencode in.
    pub project: Option<PathBuf>,
}

impl ToArgs for TuiCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        // The root command has no subcommand token; options render first.
        self.global.render(args);
        self.server.render(args);
        push_opt(args, "--model", self.model.as_deref());
        self.continuation.render(args);
        push_opt(args, "--prompt", self.prompt.as_deref());
        push_opt(args, "--agent", self.agent.as_deref());
        if let Some(project) = &self.project {
            args.push(project.into());
        }
    }
}
