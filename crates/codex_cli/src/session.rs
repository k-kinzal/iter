//! Session-lifecycle subcommands: `resume`, `fork`, `archive`, `unarchive`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt};
use crate::options::CommonConfig;
use crate::run::RunOptions;

/// `codex resume [OPTIONS] [SESSION_ID] [PROMPT]`.
///
/// Resumes an existing session in the interactive TUI. `--last` skips the
/// picker and continues the most recent session; `--all` widens the picker to
/// every recorded session. Beyond the shared [`CommonConfig`], `resume`
/// accepts the same connection/approval flags as the root run (see
/// [`RunOptions`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeCommand {
    /// Options common to the root run and its subcommands.
    pub common: CommonConfig,
    /// Connection/approval options shared with the root run.
    pub options: RunOptions,
    /// `--last`: resume the most recent session.
    pub last: bool,
    /// `--all`: include all sessions in the picker (disables cwd filtering).
    pub all: bool,
    /// `--include-non-interactive`: include non-interactive sessions in the
    /// picker and `--last` selection.
    pub include_non_interactive: bool,
    /// Optional `[SESSION_ID]` positional.
    pub session_id: Option<String>,
    /// Optional `[PROMPT]` positional seeding the resumed turn.
    pub prompt: Option<String>,
}

impl ToArgs for ResumeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("resume".into());
        self.common.render(args);
        self.options.render(args);
        push_flag(args, self.last, "--last");
        push_flag(args, self.all, "--all");
        push_flag(
            args,
            self.include_non_interactive,
            "--include-non-interactive",
        );
        if let Some(session_id) = &self.session_id {
            args.push(session_id.into());
        }
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}

/// `codex fork [OPTIONS] [SESSION_ID] [PROMPT]`.
///
/// Forks an existing session into a new one, leaving the original untouched.
/// `fork` accepts the same shared options as `resume` except
/// `--include-non-interactive`, which its `--help` does not list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkCommand {
    /// Options common to the root run and its subcommands.
    pub common: CommonConfig,
    /// Connection/approval options shared with the root run.
    pub options: RunOptions,
    /// `--last`: fork the most recent session.
    pub last: bool,
    /// `--all`: include all sessions in the picker (disables cwd filtering).
    pub all: bool,
    /// Optional `[SESSION_ID]` positional.
    pub session_id: Option<String>,
    /// Optional `[PROMPT]` positional seeding the forked turn.
    pub prompt: Option<String>,
}

impl ToArgs for ForkCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("fork".into());
        self.common.render(args);
        self.options.render(args);
        push_flag(args, self.last, "--last");
        push_flag(args, self.all, "--all");
        if let Some(session_id) = &self.session_id {
            args.push(session_id.into());
        }
        if let Some(prompt) = &self.prompt {
            args.push(prompt.into());
        }
    }
}

/// `codex archive [OPTIONS] <SESSION>`.
///
/// Archives a saved session by id or session name. The session is a **required**
/// positional; there is no `--all` flag. Beyond the shared [`CommonConfig`],
/// `archive --help` lists only the two remote-connection options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveCommand {
    /// Options common to the root run and its subcommands.
    pub common: CommonConfig,
    /// `--remote <ADDR>`: connect the TUI to a remote app server endpoint.
    pub remote: Option<String>,
    /// `--remote-auth-token-env <ENV_VAR>`.
    pub remote_auth_token_env: Option<String>,
    /// Required `<SESSION>` positional: session id (UUID) or session name.
    pub session: String,
}

impl ToArgs for ArchiveCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("archive".into());
        self.common.render(args);
        push_opt(args, "--remote", self.remote.as_deref());
        push_opt(
            args,
            "--remote-auth-token-env",
            self.remote_auth_token_env.as_deref(),
        );
        args.push((&self.session).into());
    }
}

/// `codex unarchive [OPTIONS] <SESSION>`.
///
/// Unarchives a saved session by id or session name. Same shape as
/// [`ArchiveCommand`]: a **required** `<SESSION>` positional and no `--all`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnarchiveCommand {
    /// Options common to the root run and its subcommands.
    pub common: CommonConfig,
    /// `--remote <ADDR>`: connect the TUI to a remote app server endpoint.
    pub remote: Option<String>,
    /// `--remote-auth-token-env <ENV_VAR>`.
    pub remote_auth_token_env: Option<String>,
    /// Required `<SESSION>` positional: session id (UUID) or session name.
    pub session: String,
}

impl ToArgs for UnarchiveCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("unarchive".into());
        self.common.render(args);
        push_opt(args, "--remote", self.remote.as_deref());
        push_opt(
            args,
            "--remote-auth-token-env",
            self.remote_auth_token_env.as_deref(),
        );
        args.push((&self.session).into());
    }
}
