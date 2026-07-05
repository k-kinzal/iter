//! `grok -p <PROMPT>` — the headless single-turn run.
//!
//! `grok -p/--single <PROMPT>` sends one prompt, prints the response to
//! stdout, and exits without entering the interactive UI. The prompt is the
//! *value* of the flag, delivered inline (not on stdin). Grok also accepts two
//! alternate single-turn prompt sources — `--prompt-file <PATH>` and
//! `--prompt-json <JSON>` — modeled here as [`PromptSource`].
//!
//! With `--output-format json` the whole stream is a single terminal JSON
//! object ([`SingleOutput`](crate::SingleOutput) parses it); with
//! `--output-format streaming-json` it is a newline-delimited event stream
//! ([`Grok::stream`](crate::Grok::stream) reads it).
//!
//! The behavioral flags are shared with the root run and live on
//! [`RunOptions`]; this builder adds the prompt delivery and the output-format
//! selection on top.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::ToArgs;
use crate::run::RunOptions;
use crate::values::{OutputFormat, ResumeTarget};

/// Where a headless single-turn run reads its prompt.
///
/// Grok exposes three mutually-exclusive single-turn prompt sources; the inline
/// form (`-p`) is the one iter drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSource {
    /// `-p, --single <PROMPT>` — the prompt delivered inline.
    Inline(String),
    /// `--prompt-file <PATH>` — the prompt read from a file.
    File(PathBuf),
    /// `--prompt-json <JSON>` — the prompt as JSON content blocks.
    Json(String),
}

impl PromptSource {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            // `-p <prompt>` is the headless trigger: the prompt is the *value*
            // of the flag, delivered inline (no stdin).
            Self::Inline(text) => {
                args.push("-p".into());
                args.push(text.into());
            }
            Self::File(path) => {
                args.push("--prompt-file".into());
                args.push(path.into());
            }
            Self::Json(json) => {
                args.push("--prompt-json".into());
                args.push(json.into());
            }
        }
    }
}

/// `grok -p <PROMPT> [OPTIONS]` — a headless single-turn run.
///
/// The default output format is [`OutputFormat::Plain`]. Call [`Self::json`]
/// for a machine-readable terminal object and a typed
/// [`SingleOutput`](crate::SingleOutput), or [`Self::streaming`] for the event
/// stream consumed by [`Grok::stream`](crate::Grok::stream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleCommand {
    /// Behavioral options shared with the root run.
    pub options: RunOptions,
    /// Where the prompt is read from.
    pub source: PromptSource,
    format: OutputFormat,
}

impl SingleCommand {
    /// Build a headless run whose prompt is delivered inline (`-p <PROMPT>`),
    /// defaulting to plain output.
    #[must_use]
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self::with_source(PromptSource::Inline(prompt.into()))
    }

    /// Build a headless run whose prompt is read from a file
    /// (`--prompt-file <PATH>`).
    #[must_use]
    pub fn prompt_file(path: impl Into<PathBuf>) -> Self {
        Self::with_source(PromptSource::File(path.into()))
    }

    /// Build a headless run whose prompt is JSON content blocks
    /// (`--prompt-json <JSON>`).
    #[must_use]
    pub fn prompt_json(json: impl Into<String>) -> Self {
        Self::with_source(PromptSource::Json(json.into()))
    }

    fn with_source(source: PromptSource) -> Self {
        Self {
            options: RunOptions::default(),
            source,
            format: OutputFormat::Plain,
        }
    }

    /// Set `--always-approve` (auto-approve all tool executions).
    #[must_use]
    pub fn always_approve(mut self) -> Self {
        self.options.always_approve = true;
        self
    }

    /// Set `-r, --resume` to the given target.
    #[must_use]
    pub fn resume(mut self, target: ResumeTarget) -> Self {
        self.options.resume = Some(target);
        self
    }

    /// Set `-c, --continue`.
    #[must_use]
    pub fn continue_session(mut self) -> Self {
        self.options.continue_session = true;
        self
    }

    /// The selected output format.
    #[must_use]
    pub const fn output_format(&self) -> OutputFormat {
        self.format
    }

    /// Select `--output-format json`, yielding a typed
    /// [`SingleOutput`](crate::SingleOutput) via
    /// [`Grok::execute`](crate::Grok::execute).
    #[must_use]
    pub fn json(mut self) -> JsonSingleCommand {
        self.format = OutputFormat::Json;
        JsonSingleCommand { command: self }
    }

    /// Select `--output-format streaming-json`, consumed by
    /// [`Grok::stream`](crate::Grok::stream).
    #[must_use]
    pub fn streaming(mut self) -> StreamingSingleCommand {
        self.format = OutputFormat::StreamingJson;
        StreamingSingleCommand { command: self }
    }

    fn render(&self, args: &mut Vec<OsString>) {
        // The prompt-source flag leads the argv so the machine-readable format
        // and any continuity flags follow it.
        self.source.render(args);
        args.push("--output-format".into());
        args.push(self.format.as_str().into());
        self.options.render(args);
    }
}

impl ToArgs for SingleCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.render(args);
    }
}

/// `grok -p <PROMPT> --output-format json [OPTIONS]`.
///
/// [`Grok::execute`](crate::Grok::execute) returns
/// [`SingleOutput`](crate::SingleOutput).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSingleCommand {
    command: SingleCommand,
}

impl JsonSingleCommand {
    /// Borrow the underlying single-run configuration.
    #[must_use]
    pub const fn command(&self) -> &SingleCommand {
        &self.command
    }

    /// Return the underlying single-run configuration.
    #[must_use]
    pub fn into_command(self) -> SingleCommand {
        self.command
    }
}

impl ToArgs for JsonSingleCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.render(args);
    }
}

/// `grok -p <PROMPT> --output-format streaming-json [OPTIONS]`.
///
/// Consumed by [`Grok::stream`](crate::Grok::stream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingSingleCommand {
    command: SingleCommand,
}

impl StreamingSingleCommand {
    /// Borrow the underlying single-run configuration.
    #[must_use]
    pub const fn command(&self) -> &SingleCommand {
        &self.command
    }

    /// Return the underlying single-run configuration.
    #[must_use]
    pub fn into_command(self) -> SingleCommand {
        self.command
    }
}

impl ToArgs for StreamingSingleCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        self.command.render(args);
    }
}
