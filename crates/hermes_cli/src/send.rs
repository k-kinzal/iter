//! `hermes send [OPTIONS] [message]` — deliver a message to a configured
//! messaging platform, and its `--json` result surface.
//!
//! `send` reuses the gateway's platform credentials; it runs no LLM and no
//! agent loop. With `--json` it prints a single JSON result object instead of
//! human-readable output. Its documented exit scheme is `0` ok, `1`
//! delivery/backend error, `2` usage error.
//!
//! [`SendOutput`] preserves that result object losslessly (as
//! `serde_json::Value`) and exposes typed accessors over the field names Hermes
//! is expected to emit, so an unrecognized field is retained rather than
//! dropped.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Output as ProcessOutput;

use serde_json::Value;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_path, push_opt_positional};
use crate::cli::Error;

/// `hermes send [OPTIONS] [message]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendCommand {
    /// `-t` / `--to <TARGET>`: delivery target
    /// (`platform`, `platform:chat_id`, `platform:#channel`, …).
    pub to: Option<String>,
    /// `-f` / `--file <PATH>`: read the message body from a path (`-` for
    /// stdin).
    pub file: Option<PathBuf>,
    /// `-s` / `--subject <LINE>`: prepend a subject/header line.
    pub subject: Option<String>,
    /// `-l` / `--list`: list available targets rather than sending. The
    /// [`message`](Self::message) positional then acts as an optional platform
    /// filter.
    pub list: bool,
    /// `-q` / `--quiet`: suppress stdout on success (exit code only).
    pub quiet: bool,
    /// `--json`: emit the raw JSON result instead of human-readable output.
    pub json: bool,
    /// The positional message text (or, with `--list`, an optional platform
    /// filter). When omitted the body is read from `--file` or stdin.
    pub message: Option<String>,
}

impl SendCommand {
    /// `hermes send --to <TARGET> <MESSAGE>`.
    #[must_use]
    pub fn to(target: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            to: Some(target.into()),
            message: Some(message.into()),
            ..Self::default()
        }
    }

    /// `hermes send --list [FILTER]`.
    #[must_use]
    pub fn list() -> Self {
        Self {
            list: true,
            ..Self::default()
        }
    }

    /// Return this command with `--json` requested.
    #[must_use]
    pub fn with_json(mut self) -> Self {
        self.json = true;
        self
    }
}

impl ToArgs for SendCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("send".into());
        push_opt(args, "--to", self.to.as_deref());
        push_opt_path(args, "--file", self.file.as_deref());
        push_opt(args, "--subject", self.subject.as_deref());
        push_flag(args, self.list, "--list");
        push_flag(args, self.quiet, "--quiet");
        push_flag(args, self.json, "--json");
        push_opt_positional(args, self.message.as_deref());
    }
}

/// The parsed `hermes send --json` result object.
///
/// The underlying JSON is retained verbatim; the accessors read the field
/// names Hermes is expected to use, tolerating either spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutput {
    raw: Value,
}

impl SendOutput {
    /// Parse the `--json` result from a stdout string.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Json`] when the text is not valid JSON.
    pub fn parse(stdout: &str) -> Result<Self, Error> {
        let raw = serde_json::from_str(stdout.trim())?;
        Ok(Self { raw })
    }

    /// Borrow the raw JSON result.
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.raw
    }

    /// Return the raw JSON result.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.raw
    }

    fn first_bool(&self, keys: &[&str]) -> Option<bool> {
        keys.iter()
            .find_map(|key| self.raw.get(*key).and_then(Value::as_bool))
    }

    fn first_string(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .find_map(|key| self.raw.get(*key).and_then(Value::as_str))
    }

    /// Whether delivery succeeded, from `ok` / `success` / `delivered`.
    ///
    /// Returns `None` when no such field is present, leaving the verdict to the
    /// process exit code.
    #[must_use]
    pub fn is_ok(&self) -> Option<bool> {
        self.first_bool(&["ok", "success", "delivered"])
    }

    /// The delivery target, from `target` / `to`.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.first_string(&["target", "to"])
    }

    /// The platform, from `platform`.
    #[must_use]
    pub fn platform(&self) -> Option<&str> {
        self.first_string(&["platform"])
    }

    /// The delivered message id, from `message_id` / `messageId` / `id`.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        self.first_string(&["message_id", "messageId", "id"])
    }

    /// The error detail, from `error` / `message`, when delivery failed.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.first_string(&["error", "message"])
    }
}

impl TryFrom<ProcessOutput> for SendOutput {
    type Error = Error;

    fn try_from(output: ProcessOutput) -> Result<Self, Self::Error> {
        Self::parse(&String::from_utf8_lossy(&output.stdout))
    }
}
