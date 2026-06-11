//! `iter-cron` — single-process cron trigger.
//!
//! Connects to a queue via `--queue-url`, parses `--schedule` as a cron
//! expression, and emits one signal per scheduled tick. Lives until
//! SIGTERM (or until the `--max-signals` budget is exhausted).
//!
//! Trigger flags are decomposed across [`queue_source`], [`logging`],
//! [`signal_defaults`], [`termination`], and [`banner`]. Startup and
//! shutdown banners go to stderr; stdout is reserved.

#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

mod banner;
mod cron_trigger;
mod error;
mod logging;
mod queue_source;
mod signal_defaults;
mod stream;
mod termination;

use std::sync::Arc;
use std::time::Duration;

use crate::cron_trigger::{CronTrigger, CronTriggerError};
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

const BINARY: &str = "iter-cron";

#[derive(Debug, Error)]
enum CronCliError {
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    QueueSource(#[from] QueueSourceError),
    #[error(transparent)]
    Metadata(#[from] MetadataPairError),
    #[error("installing interrupt handler: {0}")]
    Shutdown(#[source] std::io::Error),
    #[error(transparent)]
    Cron(#[from] CronTriggerError<iter_core::queue::QueueError>),
}

impl IntoExitCode for CronCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::QueueSource(e) => e.exit_code(),
            Self::Metadata(e) => e.exit_code(),
            Self::Cron(
                CronTriggerError::InvalidExpression(_) | CronTriggerError::InvalidTimezone(_),
            ) => exit_codes::USER_INPUT,
            Self::Cron(CronTriggerError::Metadata(_)) => exit_codes::INTERNAL,
            Self::Runtime(_) | Self::Shutdown(_) | Self::Cron(_) => exit_codes::RUNTIME,
        }
    }
}

/// Cron trigger with the standard `iter` flag set.
#[derive(Debug, Parser)]
#[command(
    name = BINARY,
    about = "Cron trigger that publishes signals into an iter queue",
    long_about = "iter-cron is the cron specialization of `iter trigger run`. \
                  It connects to a queue via `--queue-url`, parses `--schedule` \
                  as a cron expression, and emits one signal per scheduled tick.\n\n\
                  Startup and shutdown banners are written to stderr; stdout is \
                  reserved.",
    after_help = "EXAMPLES:\n  \
                  iter-cron --queue-url memory:// --schedule '0 * * * *'\n  \
                  iter-cron --queue-url memory:// --schedule '*/5 * * * *' --at-startup\n  \
                  iter-cron --queue-url memory:// --schedule '0 9 * * 1-5' --timezone Asia/Tokyo"
)]
struct Args {
    /// Cron expression. Five-field standard form, or six-field with seconds.
    #[arg(long, value_name = "EXPR")]
    schedule: String,

    /// IANA time zone name (e.g. `Asia/Tokyo`, `UTC`).
    #[arg(long, value_name = "TZ", default_value = "UTC")]
    timezone: String,

    /// Emit one signal immediately on startup, then enter the schedule.
    #[arg(long = "at-startup", default_value_t = false)]
    at_startup: bool,

    /// On startup, emit up to one missed tick that fell within the last
    /// N seconds. `0` disables catch-up.
    #[arg(long = "catch-up-window", value_name = "SECS", default_value_t = 0)]
    catch_up_window: u64,

    /// Maximum jitter (seconds) added uniformly at random before each tick.
    #[arg(long, value_name = "SECS", default_value_t = 0)]
    jitter: u64,

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

fn real_main() -> Result<(), CronCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(CronCliError::Runtime)?;
    runtime.block_on(run())
}

async fn run() -> Result<(), CronCliError> {
    let args = Args::parse();
    let _telemetry_guard = args.logging.init();

    let inner_queue = args.queue_source.resolve().await?;
    let cancel =
        install_signal_handlers(CancellationToken::new()).map_err(CronCliError::Shutdown)?;
    let queue = Arc::new(BudgetedQueue::new(
        inner_queue.clone(),
        args.termination.max_signals,
        cancel.clone(),
    ));

    let instance_name = args.banner.instance_name(BINARY);
    if !args.banner.quiet {
        cli_eprintln!(
            "iter-cron: started (instance=\"{}\", schedule=\"{}\", timezone=\"{}\")",
            instance_name,
            args.schedule,
            args.timezone
        );
    }

    let metadata = args.signal_defaults.base_metadata()?;
    let priority = args.signal_defaults.priority_value();
    let mut trigger = CronTrigger::new(queue.clone(), &args.schedule)?
        .with_base_metadata(metadata)
        .with_priority(priority)
        .with_at_startup(args.at_startup)
        .with_trigger_name(instance_name.clone())
        .try_with_timezone_name(&args.timezone)?;
    if args.catch_up_window > 0 {
        trigger = trigger.with_catch_up(Duration::from_secs(args.catch_up_window));
    }
    if args.jitter > 0 {
        trigger = trigger.with_jitter(Duration::from_secs(args.jitter));
    }

    let result = trigger.run(cancel.clone()).await;
    if let Err(err) = inner_queue.close().await {
        error!(error = %err, "failed to close queue cleanly");
    }
    match result {
        Ok(()) => {
            if !args.banner.quiet {
                cli_eprintln!(
                    "iter-cron: stopped (instance=\"{}\", published={})",
                    instance_name,
                    queue.published()
                );
            }
            Ok(())
        }
        Err(err) => Err(CronCliError::Cron(err)),
    }
}
