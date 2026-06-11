//! `iter-webhook` — single-process HTTP webhook trigger.
//!
//! Wraps [`WebhookTrigger`]: spawns an axum server bound to `--bind` that
//! accepts POSTs on `--path`. Each `--route PATTERN[:PRIORITY]` defines
//! a route that publishes a signal whenever an incoming event matches.
//! `--secret-env`/`--secret-file` enables HMAC-SHA256 verification.
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
mod trigger_util;
mod webhook;

use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::webhook::{Subscription, WebhookConfig, WebhookTrigger, WebhookTriggerError};
use clap::Parser;
use iter_core::{Priority, Queue};
use iter_trigger::shutdown::ShutdownError;
use iter_trigger::{CountingQueue, install_shutdown_handler};
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

const BINARY: &str = "iter-webhook";

#[derive(Debug, Error)]
enum WebhookCliError {
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("--secret-env and --secret-file are mutually exclusive")]
    SecretConflict,
    #[error("--secret-env: env var `{var}` is not set: {source}")]
    SecretEnvMissing {
        var: String,
        #[source]
        source: env::VarError,
    },
    #[error("reading secret file at {}: {source}", path.display())]
    SecretFileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("--route priority `{0}`: expected one of low, normal, high, critical")]
    RoutePriority(String),
    #[error(transparent)]
    QueueSource(#[from] QueueSourceError),
    #[error(transparent)]
    Metadata(#[from] MetadataParseError),
    #[error(transparent)]
    Shutdown(#[from] ShutdownError),
    #[error(transparent)]
    Webhook(#[from] WebhookTriggerError<iter_core::queue::QueueError>),
}

impl IntoExitCode for WebhookCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Webhook(WebhookTriggerError::Metadata(_)) => exit_codes::INTERNAL,
            Self::Runtime(_) | Self::Shutdown(_) | Self::Webhook(_) => exit_codes::RUNTIME,
            // `--secret-file FILE` failure mode depends on *why* the read
            // failed: a missing or malformed path is a user mistake, but a
            // permission denied / I/O error / disk failure is an
            // environment problem an operator scripts around differently.
            Self::SecretFileRead { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
                    exit_codes::USER_INPUT
                }
                _ => exit_codes::RUNTIME,
            },
            Self::SecretConflict | Self::SecretEnvMissing { .. } | Self::RoutePriority(_) => {
                exit_codes::USER_INPUT
            }
            Self::QueueSource(e) => e.exit_code(),
            Self::Metadata(e) => e.exit_code(),
        }
    }
}

/// HTTP webhook trigger CLI.
#[derive(Debug, Parser)]
#[command(
    name = BINARY,
    about = "Receive HTTP webhooks and publish iter signals per matched route",
    long_about = "iter-webhook is the HTTP-webhook specialization of `iter trigger run`. \
                  It binds an HTTP server on `--bind` and accepts POSTs on `--path`. \
                  Each request is matched against the declared `--route` patterns and a \
                  signal is published per match.\n\n\
                  HMAC-SHA256 verification is enabled by either `--secret-env VAR` \
                  (read the secret from an environment variable) or `--secret-file FILE` \
                  (read it from disk). The two are mutually exclusive.\n\n\
                  Startup and shutdown banners are written to stderr; stdout is reserved.",
    after_help = "EXAMPLES:\n  \
                  iter-webhook --queue-url memory://\n  \
                  iter-webhook --queue-url memory:// --bind 0.0.0.0:8080 --path /hook \\\n    \
                  --route 'push:high' --route 'issues.opened'\n  \
                  iter-webhook --queue-url memory:// --secret-env GH_WEBHOOK_SECRET"
)]
struct Args {
    /// Bind address (e.g. `0.0.0.0:8080`).
    #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,

    /// HTTP path to serve.
    #[arg(long, value_name = "PATH", default_value = "/webhook")]
    path: String,

    /// Environment variable holding the HMAC-SHA256 secret used to verify
    /// `X-Hub-Signature-256`.
    #[arg(
        long = "secret-env",
        value_name = "VAR",
        conflicts_with = "secret_file"
    )]
    secret_env: Option<String>,

    /// File containing the HMAC-SHA256 secret. Trailing whitespace is
    /// trimmed.
    #[arg(long = "secret-file", value_name = "FILE")]
    secret_file: Option<PathBuf>,

    /// Routes accepted by the server. Each entry is `PATTERN[:PRIORITY]`,
    /// e.g. `issues.opened` or `push:high`. Repeat to declare more than
    /// one. When empty, a catch-all route `*` with `normal` priority is
    /// installed.
    #[arg(long = "route", value_name = "PATTERN[:PRIORITY]")]
    route: Vec<String>,

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

fn real_main() -> Result<(), WebhookCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(WebhookCliError::Runtime)?;
    runtime.block_on(run())
}

async fn run() -> Result<(), WebhookCliError> {
    let args = Args::parse();
    let _telemetry_guard = args.logging.init();

    let secret = resolve_secret(&args)?;
    let metadata_pairs: Vec<(String, String)> = args
        .signal_shape
        .base_metadata_pairs()?
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect();
    let routes = parse_routes(&args.route, &metadata_pairs)?;

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
            "iter-webhook: started (instance=\"{}\", bind={}, path=\"{}\", routes={}, secret={})",
            instance_name,
            args.bind,
            args.path,
            routes.len(),
            secret.is_some()
        );
    }

    let config = WebhookConfig {
        bind: args.bind,
        path: args.path.clone(),
        secret,
        routes,
    };
    let trigger =
        WebhookTrigger::new(queue.clone(), config)?.with_trigger_name(instance_name.clone());

    let result = trigger.run(cancel.clone()).await;
    if let Err(err) = inner_queue.close().await {
        error!(error = %err, "failed to close queue cleanly");
    }
    match result {
        Ok(()) => {
            if !args.banner.quiet {
                cli_eprintln!(
                    "iter-webhook: stopped (instance=\"{}\", published={})",
                    instance_name,
                    queue.published()
                );
            }
            Ok(())
        }
        Err(err) => Err(WebhookCliError::Webhook(err)),
    }
}

fn resolve_secret(args: &Args) -> Result<Option<String>, WebhookCliError> {
    match (&args.secret_env, &args.secret_file) {
        (Some(_), Some(_)) => Err(WebhookCliError::SecretConflict),
        (None, None) => Ok(None),
        (Some(var), None) => {
            let value = env::var(var).map_err(|source| WebhookCliError::SecretEnvMissing {
                var: var.clone(),
                source,
            })?;
            Ok(Some(value))
        }
        (None, Some(path)) => {
            // Preflight: a directory or other non-file target is a user
            // mistake (`--secret-file /etc/`), which we want classified as
            // USER_INPUT (1). Doing this without a preflight would require
            // depending on `io::ErrorKind::IsADirectory` — recently
            // stabilised but still inconsistently surfaced across
            // platforms. The preflight collapses every "wrong path type"
            // case to `InvalidInput`, which the `IntoExitCode` impl below
            // already maps to USER_INPUT.
            if !path.is_file() {
                return Err(WebhookCliError::SecretFileRead {
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path does not point to a regular file",
                    ),
                });
            }
            let raw =
                fs::read_to_string(path).map_err(|source| WebhookCliError::SecretFileRead {
                    path: path.clone(),
                    source,
                })?;
            Ok(Some(raw.trim().to_owned()))
        }
    }
}

fn parse_routes(
    raw: &[String],
    base_metadata: &[(String, String)],
) -> Result<Vec<Subscription>, WebhookCliError> {
    if raw.is_empty() {
        return Ok(vec![Subscription {
            event_pattern: "*".into(),
            when: None,
            priority: Priority::NORMAL,
            metadata: base_metadata.to_vec(),
        }]);
    }
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let (pattern, priority) = match entry.rsplit_once(':') {
            Some((p, prio)) if !p.is_empty() => (p.to_owned(), parse_priority(prio)?),
            _ => (entry.to_owned(), Priority::NORMAL),
        };
        out.push(Subscription {
            event_pattern: pattern,
            when: None,
            priority,
            metadata: base_metadata.to_vec(),
        });
    }
    Ok(out)
}

fn parse_priority(s: &str) -> Result<Priority, WebhookCliError> {
    Ok(match s {
        "low" => Priority::LOW,
        "normal" => Priority::NORMAL,
        "high" => Priority::HIGH,
        "critical" => Priority::CRITICAL,
        _ => return Err(WebhookCliError::RoutePriority(s.to_owned())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_routes_default_is_wildcard() {
        let routes = parse_routes(&[], &[]).expect("default");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].event_pattern, "*");
    }

    #[test]
    fn parse_routes_pattern_only() {
        let routes = parse_routes(&["push".into()], &[]).expect("ok");
        assert_eq!(routes[0].event_pattern, "push");
        assert_eq!(routes[0].priority, Priority::NORMAL);
    }

    #[test]
    fn parse_routes_with_priority() {
        let routes = parse_routes(&["issues.opened:high".into()], &[]).expect("ok");
        assert_eq!(routes[0].event_pattern, "issues.opened");
        assert_eq!(routes[0].priority, Priority::HIGH);
    }

    #[test]
    fn parse_routes_rejects_unknown_priority() {
        let err = parse_routes(&["push:rocket".into()], &[]).expect_err("must fail");
        assert!(err.to_string().contains("priority"));
    }

    #[test]
    fn parse_routes_carries_base_metadata() {
        let base = vec![("source".into(), "smoke".into())];
        let routes = parse_routes(&["push".into()], &base).expect("ok");
        assert_eq!(routes[0].metadata, base);
    }
}
