//! Compose trigger construction and in-process execution.
//!
//! Each trigger kind declared in `compose.iter` is built from its
//! [`TriggerDecl`] and run as a tokio task inside the compose
//! orchestrator. The concrete trigger implementations live in the
//! standalone CLI crates (`iter_cron_cli`, `iter_watch_cli`, etc.);
//! this module wires them into the compose runtime.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iter_command_cli::{CommandTrigger, ExtractMode, OnError};
use iter_core::{Metadata, MetadataKey, MetadataValue, Priority, Queue, Signal};
use iter_cron_cli::CronTrigger;
use iter_files_cli::{FilesSource, FilesTrigger};
use iter_language::{ExtractExpr, NamedTrigger, OnErrorKeyword, PriorityKeyword, TriggerDecl};
use iter_watch_cli::{WatchConfig, WatchTrigger};
use iter_webhook_cli::{WebhookConfig, WebhookRoute as WebhookRouteConfig, WebhookTrigger};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::queue::AnyQueue;
use crate::secrets::resolve_secret;

use super::error::ComposeError;

/// A trigger ready for in-process execution by [`super::run`].
pub(crate) struct ComposeTrigger {
    pub(crate) name: String,
    pub(crate) decl: TriggerDecl,
    pub(crate) queue: Arc<AnyQueue>,
    pub(crate) terminate_on_completion: bool,
}

impl std::fmt::Debug for ComposeTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeTrigger")
            .field("name", &self.name)
            .field("terminate_on_completion", &self.terminate_on_completion)
            .finish_non_exhaustive()
    }
}

/// Error produced while running a trigger task.
#[derive(Debug, Error)]
pub enum TriggerRunError {
    /// Building the trigger from its declaration failed.
    #[error("{0}")]
    Build(Box<dyn std::error::Error + Send + Sync>),
    /// The trigger's `run()` method returned an error.
    #[error("{0}")]
    Run(Box<dyn std::error::Error + Send + Sync>),
    /// Enqueueing the post-completion terminate signal failed.
    #[error("enqueuing terminate signal: {0}")]
    Terminate(Box<dyn std::error::Error + Send + Sync>),
}

/// Build a [`ComposeTrigger`] from a parsed [`NamedTrigger`] declaration.
pub(crate) fn build_trigger(
    named: &NamedTrigger,
    queues: &std::collections::BTreeMap<String, Arc<AnyQueue>>,
) -> Result<ComposeTrigger, ComposeError> {
    if let TriggerDecl::External { ref name, .. } = named.decl {
        return Err(ComposeError::UnsupportedTriggerKind {
            trigger_name: named.name.clone(),
            kind: name.clone(),
        });
    }
    let queue = super::plan::lookup_queue(&named.target, queues)?;
    Ok(ComposeTrigger {
        name: named.name.clone(),
        decl: named.decl.clone(),
        queue,
        terminate_on_completion: named.terminate_on_completion,
    })
}

/// Run a trigger to completion, then optionally enqueue a terminate signal.
pub(crate) async fn run_trigger(
    trigger: ComposeTrigger,
    cancel: CancellationToken,
) -> Result<(), TriggerRunError> {
    let ComposeTrigger {
        name,
        decl,
        queue,
        terminate_on_completion,
    } = trigger;

    info!(trigger = %name, "starting compose trigger");

    let result = dispatch_trigger(&name, decl, queue.clone(), cancel.clone()).await;

    if let Err(ref err) = result {
        warn!(trigger = %name, error = %err, "compose trigger exited with error");
    } else {
        info!(trigger = %name, "compose trigger exited cleanly");
    }

    if result.is_ok() && terminate_on_completion && !cancel.is_cancelled() {
        let signal = Signal::terminate();
        queue
            .queue(signal, Priority::CRITICAL)
            .await
            .map_err(|e| TriggerRunError::Terminate(Box::new(e)))?;
        info!(trigger = %name, "enqueued terminate signal on target queue");
    }

    result
}

async fn dispatch_trigger(
    name: &str,
    decl: TriggerDecl,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
) -> Result<(), TriggerRunError> {
    match decl {
        TriggerDecl::Cron {
            schedule,
            timezone,
            at_startup,
            catch_up_secs,
            jitter_secs,
            base_metadata,
            priority,
            max_signals: _,
        } => {
            dispatch_cron(
                name,
                queue,
                cancel,
                schedule,
                timezone,
                at_startup,
                catch_up_secs,
                jitter_secs,
                &base_metadata,
                priority,
            )
            .await
        }
        TriggerDecl::Watch {
            dir,
            include,
            exclude,
            per_file,
            interval_secs,
            base_metadata,
            priority,
            max_signals: _,
        } => {
            dispatch_watch(
                name,
                queue,
                cancel,
                dir,
                include,
                exclude,
                per_file,
                interval_secs,
                &base_metadata,
                priority,
            )
            .await
        }
        TriggerDecl::Command {
            run: command,
            shell,
            extract,
            poll_secs,
            dedupe,
            on_error,
            base_metadata,
            priority,
            max_signals: _,
        } => {
            dispatch_command(
                name,
                queue,
                cancel,
                command,
                shell,
                extract,
                poll_secs,
                dedupe,
                on_error,
                &base_metadata,
                priority,
            )
            .await
        }
        TriggerDecl::Files {
            sources,
            no_exit_on_eof,
            base_metadata,
            priority,
            max_signals: _,
        } => {
            dispatch_files(
                name,
                queue,
                cancel,
                sources,
                no_exit_on_eof,
                &base_metadata,
                priority,
            )
            .await
        }
        TriggerDecl::Webhook {
            host,
            port,
            bind,
            path,
            secret,
            routes,
            base_metadata,
            priority,
            max_signals: _,
        } => {
            dispatch_webhook(
                name,
                queue,
                cancel,
                host,
                port,
                bind,
                path,
                secret,
                routes,
                &base_metadata,
                priority,
            )
            .await
        }
        TriggerDecl::Loop { .. } => Err(TriggerRunError::Build(Box::from(format!(
            "trigger `{name}`: loop triggers are not supported in compose; use runner.behavior = loop"
        )))),
        TriggerDecl::External { name: kind, .. } => {
            Err(TriggerRunError::Build(Box::from(format!(
                "trigger `{name}`: external trigger type `{kind}` is not supported in compose runtime"
            ))))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_cron(
    name: &str,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
    schedule: String,
    timezone: Option<String>,
    at_startup: bool,
    catch_up_secs: Option<i64>,
    jitter_secs: Option<i64>,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
) -> Result<(), TriggerRunError> {
    let metadata = build_metadata(base_metadata);
    let mut trigger = CronTrigger::new(queue, &schedule)
        .map_err(|e| TriggerRunError::Build(Box::new(e)))?
        .with_base_metadata(metadata)
        .with_priority(convert_priority(priority))
        .with_trigger_name(name)
        .with_at_startup(at_startup);
    if let Some(tz) = timezone {
        trigger = trigger
            .try_with_timezone_name(&tz)
            .map_err(|e| TriggerRunError::Build(Box::new(e)))?;
    }
    if let Some(secs) = catch_up_secs {
        let secs = u64::try_from(secs).unwrap_or(0);
        if secs > 0 {
            trigger = trigger.with_catch_up(Duration::from_secs(secs));
        }
    }
    if let Some(secs) = jitter_secs {
        let secs = u64::try_from(secs).unwrap_or(0);
        if secs > 0 {
            trigger = trigger.with_jitter(Duration::from_secs(secs));
        }
    }
    trigger
        .run(cancel)
        .await
        .map_err(|e| TriggerRunError::Run(Box::new(e)))
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_watch(
    name: &str,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
    dir: String,
    include: Vec<String>,
    exclude: Vec<String>,
    per_file: bool,
    interval_secs: Option<i64>,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
) -> Result<(), TriggerRunError> {
    let interval = interval_secs
        .and_then(|s| u64::try_from(s).ok())
        .map(Duration::from_secs);
    let config = WatchConfig::new(PathBuf::from(&dir), &include, &exclude, per_file, interval)
        .map_err(|e| TriggerRunError::Build(Box::new(e)))?;
    let metadata = build_metadata(base_metadata);
    let trigger = WatchTrigger::new(queue, config)
        .with_base_metadata(metadata)
        .with_priority(convert_priority(priority))
        .with_trigger_name(name);
    trigger
        .run(cancel)
        .await
        .map_err(|e| TriggerRunError::Run(Box::new(e)))
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_command(
    name: &str,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
    command: String,
    shell: Option<String>,
    extract: Option<ExtractExpr>,
    poll_secs: Option<i64>,
    dedupe: bool,
    on_error: Option<OnErrorKeyword>,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
) -> Result<(), TriggerRunError> {
    let extract_mode = match extract {
        Some(ExtractExpr::Regex(pat)) => ExtractMode::Regex(pat),
        None => ExtractMode::Lines,
    };
    let shell_str = shell.unwrap_or_else(|| "sh -c".to_owned());
    let poll = poll_secs
        .and_then(|s| u64::try_from(s).ok())
        .map_or_else(|| Duration::from_secs(60), Duration::from_secs);
    let on_err = match on_error {
        Some(OnErrorKeyword::Continue) | None => OnError::Continue,
        Some(OnErrorKeyword::Abort) => OnError::Abort,
        Some(OnErrorKeyword::Skip) => OnError::Skip,
    };
    let metadata = build_metadata(base_metadata);
    let trigger = CommandTrigger::new(queue, command, shell_str, extract_mode, poll)
        .with_deduplicate(dedupe)
        .with_on_error(on_err)
        .with_base_metadata(metadata)
        .with_priority(convert_priority(priority))
        .with_trigger_name(name);
    trigger
        .run(cancel)
        .await
        .map_err(|e| TriggerRunError::Run(Box::new(e)))
}

async fn dispatch_files(
    name: &str,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
    sources: Vec<iter_language::FilesSource>,
    no_exit_on_eof: bool,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
) -> Result<(), TriggerRunError> {
    let metadata = build_metadata(base_metadata);
    let p = convert_priority(priority);
    for lang_source in &sources {
        let source = match lang_source {
            iter_language::FilesSource::Stdin => FilesSource::Stdin,
            iter_language::FilesSource::Path(s) => FilesSource::Path(PathBuf::from(s)),
        };
        let trigger = FilesTrigger::new(queue.clone(), source)
            .with_base_metadata(metadata.clone())
            .with_priority(p)
            .with_trigger_name(name);
        trigger
            .run(cancel.clone())
            .await
            .map_err(|e| TriggerRunError::Run(Box::new(e)))?;
    }
    if no_exit_on_eof && !cancel.is_cancelled() {
        cancel.cancelled().await;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_webhook(
    name: &str,
    queue: Arc<AnyQueue>,
    cancel: CancellationToken,
    host: Option<String>,
    port: Option<i64>,
    bind: Option<String>,
    path: String,
    secret: Option<iter_language::SecretExpr>,
    routes: Vec<iter_language::WebhookRoute>,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
) -> Result<(), TriggerRunError> {
    let bind_addr: SocketAddr = if let Some(ref b) = bind {
        b.parse()
            .map_err(|e: std::net::AddrParseError| TriggerRunError::Build(Box::new(e)))?
    } else {
        let h = host.as_deref().unwrap_or("0.0.0.0");
        let raw_port = port.unwrap_or(8080);
        let p = u16::try_from(raw_port).map_err(|_| {
            TriggerRunError::Build(Box::from(format!(
                "webhook port {raw_port} is out of range for u16"
            )))
        })?;
        SocketAddr::new(
            h.parse()
                .map_err(|e: std::net::AddrParseError| TriggerRunError::Build(Box::new(e)))?,
            p,
        )
    };
    let resolved_secret = secret
        .as_ref()
        .map(resolve_secret)
        .transpose()
        .map_err(|e| TriggerRunError::Build(Box::new(e)))?;
    let default_priority = convert_priority(priority);
    let webhook_routes: Vec<WebhookRouteConfig> = routes
        .into_iter()
        .map(|r| {
            let route_priority = r
                .priority
                .map_or(default_priority, |kw| convert_priority(Some(kw)));
            let mut route_metadata = base_metadata.to_vec();
            route_metadata.extend(r.metadata);
            WebhookRouteConfig {
                event_pattern: r.event_pattern,
                when: r.when,
                priority: route_priority,
                metadata: route_metadata,
            }
        })
        .collect();
    let config = WebhookConfig {
        bind: bind_addr,
        path,
        secret: resolved_secret,
        routes: webhook_routes,
    };
    let trigger = WebhookTrigger::new(queue, config)
        .map_err(|e| TriggerRunError::Build(Box::new(e)))?
        .with_trigger_name(name);
    trigger
        .run(cancel)
        .await
        .map_err(|e| TriggerRunError::Run(Box::new(e)))
}

fn convert_priority(kw: Option<PriorityKeyword>) -> Priority {
    match kw {
        Some(PriorityKeyword::Low) => Priority::LOW,
        Some(PriorityKeyword::Normal) | None => Priority::NORMAL,
        Some(PriorityKeyword::High) => Priority::HIGH,
        Some(PriorityKeyword::Critical) => Priority::CRITICAL,
    }
}

fn build_metadata(pairs: &[(String, String)]) -> Metadata {
    let mut m = Metadata::new();
    for (k, v) in pairs {
        match MetadataKey::new(k) {
            Ok(key) => {
                m.insert(key, MetadataValue::String(v.clone()));
            }
            Err(e) => {
                warn!(key = %k, error = %e, "invalid metadata key in trigger declaration; skipping");
            }
        }
    }
    m
}
