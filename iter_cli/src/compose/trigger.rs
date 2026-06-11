//! Compose trigger construction and in-process execution.
//!
//! Each trigger kind declared in `compose.iter` is built from its
//! [`TriggerDef`] and run as a tokio task inside the compose
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
use iter_language::{
    ExtractExpr, NamedTrigger, OnErrorKeyword, PriorityKeyword, TriggerDef, WatchEventKind,
};
use iter_watch_cli::{ChangeKind, WatchConfig, WatchTrigger};
use iter_webhook_cli::{Subscription as SubscriptionConfig, WebhookConfig, WebhookTrigger};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::secrets::resolve_secret;

use super::error::ComposeError;

/// A trigger ready for in-process execution by [`super::run`].
#[derive(Clone)]
pub(crate) struct ComposeTrigger {
    pub(crate) name: String,
    pub(crate) decl: TriggerDef,
    pub(crate) queue: Arc<dyn Queue>,
    pub(crate) terminate_on_completion: bool,
    pub(crate) state_dir: Option<PathBuf>,
}

impl ComposeTrigger {
    /// Returns `true` when this trigger kind can complete normally without
    /// being considered an unexpected exit.  Currently only `files` without
    /// `no_exit_on_eof` is finite.
    pub(crate) fn is_finite(&self) -> bool {
        matches!(
            self.decl,
            TriggerDef::Files {
                no_exit_on_eof: false,
                ..
            }
        )
    }

    /// Human-readable kind name for status reporting.
    pub(crate) fn kind_name(&self) -> &'static str {
        match &self.decl {
            TriggerDef::Cron { .. } => "cron",
            TriggerDef::Watch { .. } => "watch",
            TriggerDef::Command { .. } => "command",
            TriggerDef::Files { .. } => "files",
            TriggerDef::Webhook { .. } => "webhook",
            TriggerDef::Loop { .. } => "loop",
            TriggerDef::External { .. } => "external",
        }
    }
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
    queues: &std::collections::BTreeMap<String, Arc<dyn Queue>>,
) -> Result<ComposeTrigger, ComposeError> {
    if let TriggerDef::External { ref name, .. } = named.decl {
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
        state_dir: None,
    })
}

/// Run a single attempt of a trigger to completion (no terminate logic).
///
/// The caller (the supervisor) is responsible for deciding whether to
/// enqueue a terminate signal after the trigger exits.
pub(crate) async fn run_trigger_once(
    trigger: &ComposeTrigger,
    cancel: CancellationToken,
) -> Result<(), TriggerRunError> {
    let name = &trigger.name;
    let result = dispatch_trigger(
        name,
        trigger.decl.clone(),
        trigger.queue.clone(),
        cancel,
        trigger.state_dir.clone(),
    )
    .await;

    if let Err(ref err) = result {
        warn!(trigger = %name, error = %err, "compose trigger exited with error");
    } else {
        info!(trigger = %name, "compose trigger exited cleanly");
    }

    result
}

/// Enqueue a terminate signal on the trigger's target queue.
pub(crate) async fn enqueue_terminate(trigger: &ComposeTrigger) -> Result<(), TriggerRunError> {
    let signal = Signal::terminate();
    trigger
        .queue
        .enqueue(signal, Priority::CRITICAL)
        .await
        .map_err(|e| TriggerRunError::Terminate(Box::new(e)))?;
    info!(trigger = %trigger.name, "enqueued terminate signal on target queue");
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn dispatch_trigger(
    name: &str,
    decl: TriggerDef,
    queue: Arc<dyn Queue>,
    cancel: CancellationToken,
    state_dir: Option<PathBuf>,
) -> Result<(), TriggerRunError> {
    match decl {
        TriggerDef::Cron {
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
        TriggerDef::Watch {
            dir,
            include,
            exclude,
            kinds,
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
                kinds,
                per_file,
                interval_secs,
                &base_metadata,
                priority,
                state_dir.clone(),
            )
            .await
        }
        TriggerDef::Command {
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
        TriggerDef::Files {
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
                state_dir,
            )
            .await
        }
        TriggerDef::Webhook {
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
        TriggerDef::Loop { .. } => Err(TriggerRunError::Build(Box::from(format!(
            "trigger `{name}`: loop triggers are not supported in compose; use runner.behavior = loop"
        )))),
        TriggerDef::External { name: kind, .. } => Err(TriggerRunError::Build(Box::from(format!(
            "trigger `{name}`: external trigger type `{kind}` is not supported in compose runtime"
        )))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_cron(
    name: &str,
    queue: Arc<dyn Queue>,
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
    queue: Arc<dyn Queue>,
    cancel: CancellationToken,
    dir: String,
    include: Vec<String>,
    exclude: Vec<String>,
    kinds: Vec<WatchEventKind>,
    per_file: bool,
    interval_secs: Option<i64>,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
    state_dir: Option<PathBuf>,
) -> Result<(), TriggerRunError> {
    let interval = interval_secs
        .and_then(|s| u64::try_from(s).ok())
        .map(Duration::from_secs);
    let allowed_kinds = convert_watch_kinds(&kinds);
    let config = WatchConfig::new(
        PathBuf::from(&dir),
        &include,
        &exclude,
        per_file,
        interval,
        allowed_kinds,
    )
    .map_err(|e| TriggerRunError::Build(Box::new(e)))?;
    let metadata = build_metadata(base_metadata);
    let mut trigger = WatchTrigger::new(queue, config)
        .with_base_metadata(metadata)
        .with_priority(convert_priority(priority))
        .with_trigger_name(name);
    if let Some(dir) = state_dir {
        trigger = trigger.with_state_dir(dir);
    }
    trigger
        .run(cancel)
        .await
        .map_err(|e| TriggerRunError::Run(Box::new(e)))
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_command(
    name: &str,
    queue: Arc<dyn Queue>,
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

#[allow(clippy::too_many_arguments)]
async fn dispatch_files(
    name: &str,
    queue: Arc<dyn Queue>,
    cancel: CancellationToken,
    sources: Vec<iter_language::FilesSource>,
    no_exit_on_eof: bool,
    base_metadata: &[(String, String)],
    priority: Option<PriorityKeyword>,
    state_dir: Option<PathBuf>,
) -> Result<(), TriggerRunError> {
    let metadata = build_metadata(base_metadata);
    let p = convert_priority(priority);
    for (idx, lang_source) in sources.iter().enumerate() {
        let source = match lang_source {
            iter_language::FilesSource::Stdin => FilesSource::Stdin,
            iter_language::FilesSource::Path(s) => FilesSource::Path(PathBuf::from(s)),
        };
        let mut trigger = FilesTrigger::new(queue.clone(), source)
            .with_base_metadata(metadata.clone())
            .with_priority(p)
            .with_trigger_name(name);
        if let Some(ref dir) = state_dir {
            trigger = trigger.with_state_dir(dir.join(format!("source_{idx}")));
        }
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
    queue: Arc<dyn Queue>,
    cancel: CancellationToken,
    host: Option<String>,
    port: Option<i64>,
    bind: Option<String>,
    path: String,
    secret: Option<iter_language::SecretExpr>,
    routes: Vec<iter_language::Subscription>,
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
    let webhook_routes: Vec<SubscriptionConfig> = routes
        .into_iter()
        .map(|r| {
            let route_priority = r
                .priority
                .map_or(default_priority, |kw| convert_priority(Some(kw)));
            let mut route_metadata = base_metadata.to_vec();
            route_metadata.extend(r.metadata);
            SubscriptionConfig {
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

fn convert_watch_kinds(kinds: &[WatchEventKind]) -> std::collections::HashSet<ChangeKind> {
    kinds
        .iter()
        .map(|k| match k {
            WatchEventKind::Created => ChangeKind::Created,
            WatchEventKind::Modified => ChangeKind::Modified,
            WatchEventKind::Removed => ChangeKind::Removed,
        })
        .collect()
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
