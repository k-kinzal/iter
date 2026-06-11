//! `iter-files` — single-process trigger that publishes one signal per
//! line read from stdin or a file.
//!
//! Each `--from` source is drained in order. Empty lines and lines
//! beginning with `#` are skipped (matching [`FilesTrigger`]'s built-in
//! behaviour).
//!
//! Startup and shutdown banners go to stderr; stdout is reserved.

#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

mod banner;
mod error;
mod files_trigger;
mod logging;
mod queue_source;
mod signal_defaults;
mod stream;
mod termination;

use std::path::PathBuf;
use std::sync::Arc;

use crate::files_trigger::{FilesSource, FilesTrigger, FilesTriggerError};
use clap::Parser;
use iter_core::process::interrupt::install_signal_handlers;
use iter_core::queue::BudgetedQueue;
use iter_core::signal::defaults::MetadataPairError;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::banner::BannerArgs;
use crate::error::{IntoExitCode, exit_codes, run_main};
use crate::logging::LoggingArgs;
use crate::queue_source::{QueueSourceArgs, QueueSourceError};
use crate::signal_defaults::SignalDefaultsArgs;
use crate::stream::cli_eprintln;
use crate::termination::TerminationArgs;

const BINARY: &str = "iter-files";

#[derive(Debug, Error)]
enum FilesCliError {
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    QueueSource(#[from] QueueSourceError),
    #[error(transparent)]
    Metadata(#[from] MetadataPairError),
    #[error("installing interrupt handler: {0}")]
    Shutdown(#[source] std::io::Error),
    #[error(transparent)]
    Files(#[from] FilesTriggerError<iter_core::queue::QueueError>),
    #[error("--from path: must include a path after the colon")]
    SourceMissingPath,
    #[error("--from `{0}`: expected `stdin` or `path:<file>`")]
    SourceUnknownForm(String),
}

impl IntoExitCode for FilesCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Files(FilesTriggerError::Metadata(_)) => exit_codes::INTERNAL,
            Self::Runtime(_) | Self::Shutdown(_) | Self::Files(_) => exit_codes::RUNTIME,
            Self::QueueSource(e) => e.exit_code(),
            Self::Metadata(e) => e.exit_code(),
            Self::SourceMissingPath | Self::SourceUnknownForm(_) => exit_codes::USER_INPUT,
        }
    }
}

/// Files trigger CLI.
#[derive(Debug, Parser)]
#[command(
    name = BINARY,
    about = "Publish one signal per line from stdin or a file",
    long_about = "iter-files is the file-list specialization of `iter trigger run`. \
                  It drains each `--from` source in order and publishes one signal per \
                  non-empty, non-comment line.\n\n\
                  Sources: `stdin` (or `-`) reads standard input; `path:PATH` reads from \
                  a file. Repeat `--from` to chain sources.\n\n\
                  Startup and shutdown banners are written to stderr; stdout is reserved.",
    after_help = "EXAMPLES:\n  \
                  echo backlog | iter-files --queue-url memory://\n  \
                  iter-files --queue-url memory:// --from path:tasks.txt --from stdin\n  \
                  iter-files --queue-url file:///tmp/q --from path:list.txt --no-exit-on-eof"
)]
struct Args {
    /// Source(s) to drain, in order. Either `stdin` or `path:PATH`.
    /// Repeat to chain multiple sources. Defaults to a single `stdin`.
    #[arg(long = "from", value_name = "SOURCE")]
    from: Vec<String>,

    /// After draining all `--from` sources, keep the process alive until
    /// SIGTERM. Default is to exit on EOF.
    #[arg(long = "no-exit-on-eof", default_value_t = false)]
    no_exit_on_eof: bool,

    #[command(flatten)]
    queue_source: QueueSourceArgs,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(flatten)]
    signal_defaults: SignalDefaultsArgs,

    #[command(flatten)]
    termination: TerminationArgs,

    #[command(flatten)]
    banner: BannerArgs,
}

fn main() -> ! {
    run_main(real_main)
}

fn real_main() -> Result<(), FilesCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(FilesCliError::Runtime)?;
    runtime.block_on(run())
}

async fn run() -> Result<(), FilesCliError> {
    let args = Args::parse();
    let _telemetry_guard = args.logging.init();

    let sources = parse_sources(&args.from)?;
    let inner_queue = args.queue_source.resolve().await?;
    let cancel =
        install_signal_handlers(CancellationToken::new()).map_err(FilesCliError::Shutdown)?;
    let queue = Arc::new(BudgetedQueue::new(
        inner_queue.clone(),
        args.termination.max_signals,
        cancel.clone(),
    ));

    let instance_name = args.banner.instance_name(BINARY);
    if !args.banner.quiet {
        cli_eprintln!(
            "iter-files: started (instance=\"{}\", sources={})",
            instance_name,
            sources.len()
        );
    }

    let metadata = args.signal_defaults.base_metadata()?;
    let priority = args.signal_defaults.priority_value();

    for source in sources {
        if cancel.is_cancelled() {
            break;
        }
        let trigger = FilesTrigger::new(queue.clone(), source.clone())
            .with_base_metadata(metadata.clone())
            .with_priority(priority)
            .with_trigger_name(instance_name.clone());
        if let Err(err) = trigger.run(cancel.clone()).await {
            cancel.cancel();
            return Err(FilesCliError::Files(err));
        }
    }

    if args.no_exit_on_eof && !cancel.is_cancelled() {
        cancel.cancelled().await;
    }

    if let Err(err) = inner_queue.close().await {
        error!(error = %err, "failed to close queue cleanly");
    }
    if !args.banner.quiet {
        cli_eprintln!(
            "iter-files: stopped (instance=\"{}\", published={})",
            instance_name,
            queue.published()
        );
    }
    Ok(())
}

fn parse_sources(raw: &[String]) -> Result<Vec<FilesSource>, FilesCliError> {
    if raw.is_empty() {
        return Ok(vec![FilesSource::Stdin]);
    }
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        out.push(parse_source(entry)?);
    }
    Ok(out)
}

fn parse_source(s: &str) -> Result<FilesSource, FilesCliError> {
    if s == "stdin" || s == "-" {
        return Ok(FilesSource::Stdin);
    }
    if let Some(path) = s.strip_prefix("path:") {
        if path.is_empty() {
            return Err(FilesCliError::SourceMissingPath);
        }
        return Ok(FilesSource::Path(PathBuf::from(path)));
    }
    Err(FilesCliError::SourceUnknownForm(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_stdin() {
        let sources = parse_sources(&[]).expect("default");
        assert!(matches!(sources.as_slice(), [FilesSource::Stdin]));
    }

    #[test]
    fn parses_path_prefix() {
        let sources = parse_sources(&["path:/tmp/list.txt".into()]).expect("path");
        assert!(matches!(sources.first(), Some(FilesSource::Path(_))));
    }

    #[test]
    fn rejects_unknown_source_form() {
        let err = parse_sources(&["http://nope".into()]).expect_err("must fail");
        assert!(err.to_string().contains("expected"));
    }

    #[test]
    fn empty_path_after_prefix_errors() {
        let err = parse_sources(&["path:".into()]).expect_err("must fail");
        assert!(err.to_string().contains("must include"));
    }
}
