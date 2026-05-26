//! `iter-watch` — single-process filesystem watch trigger.
//!
//! Wraps [`WatchTrigger`]: emits a signal whenever a file inside `--dir`
//! changes (or is created/removed). When `--per-file` is unset, events
//! arriving inside `--interval` are merged into a single signal.
//!
//! Startup and shutdown banners go to stderr; stdout is reserved.

#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

mod banner;
mod error;
mod logging;
mod queue_source;
mod signal_shape;
mod stream;
mod termination;
mod watch;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::watch::{ChangeKind, WatchConfig, WatchTrigger, WatchTriggerError};
use clap::Parser;
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

const BINARY: &str = "iter-watch";

#[derive(Debug, Error)]
enum WatchCliError {
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("watched directory does not exist: {}", path.display())]
    WatchedDirMissing { path: PathBuf },
    #[error(transparent)]
    QueueSource(#[from] QueueSourceError),
    #[error(transparent)]
    Metadata(#[from] MetadataParseError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error(transparent)]
    Watch(#[from] WatchTriggerError<QueueHandleError>),
    #[error("invalid glob pattern: {0}")]
    InvalidGlob(#[from] globset::Error),
}

impl IntoExitCode for WatchCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Watch(WatchTriggerError::Metadata(_)) => exit_codes::INTERNAL,
            Self::Runtime(_) | Self::Shutdown(_) | Self::Watch(_) => exit_codes::RUNTIME,
            Self::WatchedDirMissing { .. } | Self::InvalidGlob(_) => exit_codes::USER_INPUT,
            Self::QueueSource(e) => e.exit_code(),
            Self::Metadata(e) => e.exit_code(),
        }
    }
}

/// Filesystem watcher trigger CLI.
#[derive(Debug, Parser)]
#[command(
    name = BINARY,
    about = "Publish signals on filesystem changes inside a directory",
    long_about = "iter-watch is the filesystem-watch specialization of `iter trigger run`. \
                  It observes `--dir` for file create / modify / remove events and \
                  publishes signals on every match.\n\n\
                  By default events inside the same `--interval` window are merged into a \
                  single signal. Pass `--per-file` to emit one signal per change \
                  (with no interval applied).\n\n\
                  Startup and shutdown banners are written to stderr; stdout is reserved.",
    after_help = "EXAMPLES:\n  \
                  iter-watch --queue-url memory:// --dir ./src\n  \
                  iter-watch --queue-url memory:// --dir . --include '*.rs' --exclude 'target/**'\n  \
                  iter-watch --queue-url file:///tmp/q --dir . --per-file\n  \
                  iter-watch --queue-url file:///tmp/q --dir . --per-file --interval 5\n  \
                  iter-watch --queue-url memory:// --dir . --kinds created --kinds modified"
)]
struct Args {
    /// Directory to watch.
    #[arg(long, value_name = "DIR")]
    dir: PathBuf,

    /// Glob include filter applied to paths relative to `--dir`. May repeat.
    /// Empty means "all files".
    #[arg(long, value_name = "PATTERN")]
    include: Vec<String>,

    /// Glob exclude filter applied to paths relative to `--dir`. May repeat.
    /// Always wins over `--include`.
    #[arg(long, value_name = "PATTERN")]
    exclude: Vec<String>,

    /// Only emit signals for the specified event kinds. May repeat.
    /// Valid values: `created`, `modified`, `removed`.
    /// Empty (default) means all event kinds.
    #[arg(long, value_name = "KIND", value_parser = parse_change_kind)]
    kinds: Vec<ChangeKind>,

    /// Emit one signal per file change rather than merging by interval.
    /// When combined with `--interval`, events are still merged into
    /// interval signals.
    #[arg(long = "per-file", default_value_t = false)]
    per_file: bool,

    /// Publish interval in seconds. After the first matching event, collect
    /// changes for this duration, then emit one merged signal. All observed
    /// events are preserved — no per-path suppression.
    /// Defaults to 2 when `--per-file` is not set. With `--per-file` and
    /// no explicit interval, each event fires its own signal immediately.
    /// Pass `0` to disable the interval (per-file mode emits immediately;
    /// batched mode uses the internal 250 ms default).
    #[arg(long, value_name = "SECS")]
    interval: Option<u64>,

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

fn parse_change_kind(s: &str) -> Result<ChangeKind, String> {
    match s {
        "created" => Ok(ChangeKind::Created),
        "modified" => Ok(ChangeKind::Modified),
        "removed" => Ok(ChangeKind::Removed),
        _ => Err(format!(
            "unknown event kind `{s}`; valid values: created, modified, removed"
        )),
    }
}

fn main() -> ! {
    run_main(real_main)
}

fn real_main() -> Result<(), WatchCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(WatchCliError::Runtime)?;
    runtime.block_on(run())
}

async fn run() -> Result<(), WatchCliError> {
    let args = Args::parse();
    let _telemetry_guard = args.logging.init();

    if !args.dir.exists() {
        return Err(WatchCliError::WatchedDirMissing {
            path: args.dir.clone(),
        });
    }

    let inner_queue = Arc::new(args.queue_source.resolve().await?);
    let cancel = install_shutdown_handler(CancellationToken::new())?;
    let queue = Arc::new(CountingQueue::new(
        inner_queue.clone(),
        args.termination.max_signals,
        cancel.clone(),
    ));

    let interval = match args.interval {
        Some(0) => None,
        Some(n) => Some(Duration::from_secs(n)),
        None if args.per_file => None,
        None => Some(Duration::from_secs(2)),
    };
    // Duplicates are silently collapsed — the CLI is lenient unlike the
    // language layer, which warns on duplicate kind entries.
    let kinds: HashSet<ChangeKind> = args.kinds.iter().copied().collect();
    let config = WatchConfig::new(
        args.dir.clone(),
        &args.include,
        &args.exclude,
        args.per_file,
        interval,
        kinds,
    )?;

    let instance_name = args.banner.instance_name(BINARY);
    if !args.banner.quiet {
        cli_eprintln!(
            "iter-watch: started (instance=\"{}\", dir=\"{}\", per_file={})",
            instance_name,
            args.dir.display(),
            args.per_file
        );
    }

    let metadata = args.signal_shape.base_metadata()?;
    let priority = args.signal_shape.priority_value();
    let trigger = WatchTrigger::new(queue.clone(), config)
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
                    "iter-watch: stopped (instance=\"{}\", published={})",
                    instance_name,
                    queue.published()
                );
            }
            Ok(())
        }
        Err(err) => Err(WatchCliError::Watch(err)),
    }
}
