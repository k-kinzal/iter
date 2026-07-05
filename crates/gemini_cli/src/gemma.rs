//! `gemini gemma` — manage local Gemma model routing.
//!
//! The leaf shapes are modeled from each leaf's own `gemini gemma <leaf>
//! --help`. Every leaf carries flags over the LiteRT-LM server lifecycle:
//! `setup`/`start`/`stop`/`status` take `--port`, `setup` additionally takes
//! `--skip-model` / `--start` / `--force` / `--consent`, and `logs` takes
//! `-n/--lines` and `-f/--follow`.

use std::ffi::OsString;

use crate::args::{ToArgs, push_bool, push_flag, push_num};

/// `gemini gemma [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GemmaCommand {
    /// `-d, --debug`.
    pub debug: bool,
    /// The gemma subcommand.
    pub command: GemmaSubcommand,
}

impl GemmaCommand {
    /// Wrap a gemma subcommand with default options.
    #[must_use]
    pub fn new(command: GemmaSubcommand) -> Self {
        Self {
            debug: false,
            command,
        }
    }
}

impl ToArgs for GemmaCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("gemma".into());
        push_flag(args, self.debug, "--debug");
        self.command.render(args);
    }
}

/// A `gemini gemma` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GemmaSubcommand {
    /// `gemma setup`: download and configure Gemma local model routing.
    Setup {
        /// `--port <PORT>`: port for the LiteRT server.
        port: Option<u16>,
        /// `--skip-model`: skip model download (binary only).
        skip_model: bool,
        /// `--start` / `--no-start`: start the server after setup. Default-true
        /// on the CLI; `false` emits `--no-start` so the off-state (setup
        /// without launching the LiteRT server) is expressible.
        start: bool,
        /// `--force`: re-download binary and model even if already present.
        force: bool,
        /// `--consent`: skip interactive consent prompt (implies acceptance).
        consent: bool,
    },
    /// `gemma start`: start the LiteRT-LM server.
    Start {
        /// `--port <PORT>`: port for the LiteRT server.
        port: Option<u16>,
    },
    /// `gemma stop`: stop the LiteRT-LM server.
    Stop {
        /// `--port <PORT>`: port where the LiteRT server is running.
        port: Option<u16>,
    },
    /// `gemma status`: check Gemma local model routing status.
    Status {
        /// `--port <PORT>`: port to check for the LiteRT server.
        port: Option<u16>,
    },
    /// `gemma logs`: view LiteRT-LM server logs.
    Logs {
        /// `-n, --lines <N>`: show the last N lines and exit.
        lines: Option<u64>,
        /// `-f, --follow` / `--no-follow`: follow log output. Default-true on
        /// the CLI (when `--lines` is omitted); `false` emits `--no-follow` so
        /// the tail-without-follow state is expressible.
        follow: bool,
    },
}

impl GemmaSubcommand {
    fn render(self, args: &mut Vec<OsString>) {
        match self {
            Self::Setup {
                port,
                skip_model,
                start,
                force,
                consent,
            } => {
                args.push("setup".into());
                push_num(args, "--port", port);
                push_flag(args, skip_model, "--skip-model");
                push_bool(args, start, "--start", "--no-start");
                push_flag(args, force, "--force");
                push_flag(args, consent, "--consent");
            }
            Self::Start { port } => {
                args.push("start".into());
                push_num(args, "--port", port);
            }
            Self::Stop { port } => {
                args.push("stop".into());
                push_num(args, "--port", port);
            }
            Self::Status { port } => {
                args.push("status".into());
                push_num(args, "--port", port);
            }
            Self::Logs { lines, follow } => {
                args.push("logs".into());
                push_num(args, "--lines", lines);
                push_bool(args, follow, "--follow", "--no-follow");
            }
        }
    }
}
