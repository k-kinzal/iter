//! `iter-command` — single-process command-poll trigger.
//!
//! Runs `--run` under `--shell` on a fixed `--poll-secs` interval, applies
//! `--extract` to the captured stdout, and publishes one signal per
//! extracted record into the queue identified by `--queue-url`. Lives
//! until SIGTERM (or until the `--max-signals` budget is exhausted via
//! [`iter_trigger::CountingQueue`]).
//!
//! Trigger flags are decomposed across [`queue_source`], [`logging`],
//! [`signal_shape`], [`termination`], and [`banner`]. Startup and
//! shutdown banners go to stderr; stdout is reserved.

#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

mod banner;
mod command;
mod error;
mod logging;
mod queue_source;
mod signal_shape;
mod stream;
mod termination;

use std::sync::Arc;
use std::time::Duration;

use crate::command::{CommandTrigger, CommandTriggerError, ExtractMode, OnError};
use clap::{Parser, ValueEnum};
use iter_core::Queue;
use iter_trigger::shutdown::ShutdownError;
use iter_trigger::{CountingQueue, QueueHandleError, install_shutdown_handler};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::banner::BannerArgs;
use crate::error::{IntoExitCode, exit_codes, run_main};
use crate::logging::LoggingArgs;
use crate::queue_source::{QueueSourceArgs, QueueSourceError};
use crate::signal_shape::{MetadataParseError, SignalShapeArgs};
use crate::stream::cli_eprintln;
use crate::termination::TerminationArgs;

const BINARY: &str = "iter-command";

#[derive(Debug, Error)]
enum CommandCliError {
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    QueueSource(#[from] QueueSourceError),
    #[error(transparent)]
    Metadata(#[from] MetadataParseError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error(transparent)]
    Command(#[from] CommandTriggerError<QueueHandleError>),
    #[error("--extract `{0}`: expected `lines` or `regex:PATTERN`")]
    ExtractUnknownForm(String),
    #[error("--extract regex pattern must not be empty")]
    ExtractEmptyRegex,
}

impl IntoExitCode for CommandCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Command(CommandTriggerError::Metadata(_)) => exit_codes::INTERNAL,
            Self::Runtime(_) | Self::Shutdown(_) | Self::Command(_) => exit_codes::RUNTIME,
            Self::QueueSource(e) => e.exit_code(),
            Self::Metadata(e) => e.exit_code(),
            Self::ExtractUnknownForm(_) | Self::ExtractEmptyRegex => exit_codes::USER_INPUT,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OnErrorArg {
    Continue,
    Abort,
    Skip,
}

impl OnErrorArg {
    fn into_on_error(self) -> OnError {
        match self {
            Self::Continue => OnError::Continue,
            Self::Abort => OnError::Abort,
            Self::Skip => OnError::Skip,
        }
    }
}

/// Command-poll trigger with the standard `iter` flag set.
#[derive(Debug, Parser)]
#[command(
    name = BINARY,
    about = "Poll a shell command and publish one signal per extracted record",
    long_about = "iter-command is the command-poll specialization of `iter trigger run`. \
                  It runs `--run` under `--shell` every `--poll-secs` seconds, applies \
                  `--extract` to the captured stdout, and publishes one signal per \
                  extracted record into the queue.\n\n\
                  Extract modes: `lines` (default) emits one signal per non-empty line; \
                  `regex:PATTERN` emits one signal per regex match (PCRE2-flavoured).\n\n\
                  Startup and shutdown banners are written to stderr; stdout is reserved.",
    after_help = "EXAMPLES:\n  \
                  iter-command --queue-url memory:// --run 'ls -1 incoming/'\n  \
                  iter-command --queue-url memory:// --run 'date +%s' --poll-secs 5 --dedupe\n  \
                  iter-command --queue-url file:///tmp/q --run 'gh issue list --json number' \\\n    \
                  --extract 'regex:\"number\":\\s*(\\d+)' --on-error skip"
)]
struct Args {
    /// Shell command to run on every poll. Interpreted by `--shell`.
    #[arg(long = "run", value_name = "CMD")]
    run: String,

    /// Program/argv-prefix used to interpret `--run`. The trailing `-c`
    /// (or equivalent) is required so the command string is interpreted.
    #[arg(long, value_name = "SHELL", default_value = "sh -c")]
    shell: String,

    /// Output extraction mode. `lines` (default) or `regex:PATTERN`.
    #[arg(long, value_name = "MODE", default_value = "lines")]
    extract: String,

    /// Poll interval in seconds.
    #[arg(long = "poll-secs", value_name = "SECS", default_value_t = 60)]
    poll_secs: u64,

    /// Deduplicate records across polls (records observed in earlier
    /// polls are not re-emitted).
    #[arg(long, default_value_t = false)]
    dedupe: bool,

    /// Behaviour when the polled command exits with a non-zero status.
    #[arg(long = "on-error", value_enum, default_value_t = OnErrorArg::Continue)]
    on_error: OnErrorArg,

    #[command(flatten)]
    queue_source: QueueSourceArgs,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(flatten)]
    signal_shape: SignalShapeArgs,

    #[command(flatten)]
    termination: TerminationArgs,

    #[command(flatten)]
    banner: BannerArgs,
}

fn main() -> ! {
    run_main(real_main)
}

fn real_main() -> Result<(), CommandCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(CommandCliError::Runtime)?;
    runtime.block_on(run())
}

async fn run() -> Result<(), CommandCliError> {
    let args = Args::parse();
    let _telemetry_guard = args.logging.init();

    let extract = parse_extract(&args.extract)?;
    let inner_queue = Arc::new(args.queue_source.resolve().await?);
    let cancel = install_shutdown_handler(CancellationToken::new())?;
    let queue = Arc::new(CountingQueue::new(
        inner_queue.clone(),
        args.termination.max_signals,
        cancel.clone(),
    ));

    let instance_name = args.banner.instance_name(BINARY);
    if !args.banner.quiet {
        cli_eprintln!(
            "iter-command: started (instance=\"{}\", run=\"{}\", poll_secs={})",
            instance_name,
            args.run,
            args.poll_secs
        );
    }

    let metadata = args.signal_shape.base_metadata()?;
    let priority = args.signal_shape.priority_value();
    let trigger = CommandTrigger::new(
        queue.clone(),
        args.run.clone(),
        args.shell.clone(),
        extract,
        Duration::from_secs(args.poll_secs),
    )
    .with_deduplicate(args.dedupe)
    .with_on_error(args.on_error.into_on_error())
    .with_base_metadata(metadata)
    .with_priority(priority)
    .with_trigger_name(instance_name.clone());

    let result = trigger.run(cancel.clone()).await;
    if let Err(err) = inner_queue.close().await {
        error!(error = %err, "failed to close queue cleanly");
    }
    match result {
        Ok(()) => {
            if !args.banner.quiet {
                cli_eprintln!(
                    "iter-command: stopped (instance=\"{}\", published={})",
                    instance_name,
                    queue.published()
                );
            }
            Ok(())
        }
        Err(err) => Err(CommandCliError::Command(err)),
    }
}

fn parse_extract(s: &str) -> Result<ExtractMode, CommandCliError> {
    if s == "lines" {
        return Ok(ExtractMode::Lines);
    }
    if let Some(pattern) = s.strip_prefix("regex:") {
        if pattern.is_empty() {
            return Err(CommandCliError::ExtractEmptyRegex);
        }
        return Ok(ExtractMode::Regex(pattern.to_owned()));
    }
    Err(CommandCliError::ExtractUnknownForm(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extract_lines() {
        let mode = parse_extract("lines").expect("lines");
        assert!(matches!(mode, ExtractMode::Lines));
    }

    #[test]
    fn parse_extract_regex() {
        let mode = parse_extract("regex:\\d+").expect("regex");
        assert!(matches!(mode, ExtractMode::Regex(p) if p == "\\d+"));
    }

    #[test]
    fn parse_extract_empty_regex_errors() {
        let err = parse_extract("regex:").expect_err("must fail");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn parse_extract_unknown_errors() {
        let err = parse_extract("json").expect_err("must fail");
        assert!(err.to_string().contains("expected"));
    }
}
