//! Azure Service Bus queue backend.
//!
//! Built on the community `azservicebus` crate (AMQP 1.0). Queues,
//! topic+subscription pairs, and `dead_letter` / `transfer_dead_letter`
//! sub-queues are all reachable from the same [`ServiceBusQueue`] type.
//! `PeekLock` with immediate `complete()` on receipt provides the
//! at-most-once delivery contract documented on [`crate::Queue`].
//!
//! # Stub implementation
//!
//! Phase 3 of the queue-backend expansion lands the full DSL surface for
//! `queue servicebus { ... }` (queue / topic+subscription identity,
//! AMQP-TCP and AMQP-WebSockets transports with optional proxy, native
//! `DefaultAzureCredential` chain plus connection-string / SAS / AAD
//! variants, sender / receiver / session knobs, sub-queue DLQ consumer
//! mode), plus the lowerer, `AnyQueue` dispatch arm, and compose-layer
//! translation. The actual `azservicebus` runtime — AMQP link
//! establishment, `PeekLock` + complete cycle, session-receiver code path,
//! lock-renewal timer, on-handler-error abandon/dead-letter/defer
//! routing — lands in a follow-up release.
//!
//! Until that lands, [`ServiceBusQueue::new`] succeeds (so the runner
//! can be constructed end-to-end and the Iterfile validates) but every
//! `queue` / `dequeue` call returns
//! [`ServiceBusQueueError::NotYetImplemented`]. This matches the
//! pattern used for [`PubSubQueueError::NotYetImplemented`](super::super::gcp::pubsub::PubSubQueueError::NotYetImplemented),
//! [`KafkaQueueError::NotYetImplemented`](super::super::kafka::KafkaQueueError::NotYetImplemented),
//! and [`KinesisQueueError::NotYetImplemented`](super::super::aws::kinesis::KinesisQueueError::NotYetImplemented).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::queue::Priority;
use crate::queue::Queue;
use crate::queue::dlq::DlqPolicy;
use crate::queue::drivers::servicebus::credentials::{
    ServiceBusCredentials, ServiceBusCredentialsError,
};
use crate::queue::drivers::sqs::MetadataSource;
use crate::queue::retry::RetryPolicy;
use crate::signal::Signal;

/// Service Bus entity kind.
#[derive(Debug, Clone)]
pub enum ServiceBusEntity {
    /// Standalone queue.
    Queue {
        /// Queue name.
        name: String,
    },
    /// Subscription on a topic.
    Subscription {
        /// Topic name.
        topic: String,
        /// Subscription name.
        subscription: String,
    },
}

/// WebSocket proxy configuration. Strings are post-`SecretExpr`
/// resolution.
#[derive(Debug, Clone)]
pub struct ServiceBusProxyConfig {
    /// Proxy URL.
    pub url: String,
    /// Optional proxy username.
    pub username: Option<String>,
    /// Optional proxy password.
    pub password: Option<String>,
}

/// Sender knobs.
#[derive(Debug, Clone, Default)]
pub struct ServiceBusSenderConfig {
    /// Per-message id template.
    pub message_id: Option<MetadataSource>,
    /// Correlation id template.
    pub correlation_id: Option<MetadataSource>,
    /// Static content type.
    pub content_type: Option<String>,
    /// Static subject.
    pub subject: Option<String>,
    /// Reply-to entity name.
    pub reply_to: Option<String>,
    /// Reply-to session id.
    pub reply_to_session_id: Option<String>,
    /// Per-message TTL.
    pub time_to_live: Option<Duration>,
    /// RFC3339 scheduled enqueue time.
    pub scheduled_enqueue_time: Option<String>,
    /// Partition key source.
    pub partition_key_strategy: Option<MetadataSource>,
    /// Session id source.
    pub session_id_strategy: Option<MetadataSource>,
    /// Static application-property overlay.
    pub application_properties: Option<Vec<(String, String)>>,
    /// Batch size cap.
    pub batch_size: Option<u32>,
    /// Batch byte cap.
    pub batch_max_bytes: Option<u32>,
    /// Linger before flushing a partial batch.
    pub batch_linger: Option<Duration>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicy>,
}

/// Receiver knobs.
#[derive(Debug, Clone, Default)]
pub struct ServiceBusReceiverConfig {
    /// `peek_lock` (default) or `receive_and_delete`.
    pub receive_mode: Option<String>,
    /// Prefetch count.
    pub prefetch_count: Option<u32>,
    /// `none` (default), `dead_letter`, or `transfer_dead_letter`.
    pub sub_queue: Option<String>,
    /// Optional client identifier.
    pub identifier: Option<String>,
    /// Max wait per receive batch.
    pub max_wait_time: Option<Duration>,
    /// Max messages per receive batch.
    pub max_messages: Option<u32>,
    /// Max auto lock-renewal duration.
    pub max_auto_lock_renewal_duration: Option<Duration>,
    /// `abandon` (default), `dead_letter`, or `defer`.
    pub on_handler_error: Option<String>,
    /// DLQ reason template.
    pub dead_letter_reason_template: Option<String>,
    /// DLQ description template.
    pub dead_letter_description_template: Option<String>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicy>,
}

/// Session knobs.
#[derive(Debug, Clone, Default)]
pub struct ServiceBusSessionConfig {
    /// `accept_specific` or `accept_next`.
    pub mode: Option<String>,
    /// Required when `mode = "accept_specific"`.
    pub session_id: Option<String>,
    /// Idle timeout before releasing the session.
    pub session_idle_timeout: Option<Duration>,
}

/// Resolved Service Bus queue configuration. Compose-layer
/// responsibility to produce this from the AST (resolving `SecretExpr`,
/// etc.) before calling [`ServiceBusQueue::new`].
#[derive(Debug, Clone)]
pub struct ServiceBusQueueConfig {
    /// Fully-qualified namespace.
    pub fully_qualified_namespace: Option<String>,
    /// Entity (queue or topic+subscription).
    pub entity: ServiceBusEntity,
    /// `amqp_tcp` (default) or `amqp_websockets`.
    pub transport: Option<String>,
    /// Optional private-endpoint host.
    pub custom_endpoint_address: Option<String>,
    /// WebSocket proxy.
    pub web_proxy: Option<ServiceBusProxyConfig>,
    /// Connection idle timeout.
    pub connection_idle_timeout: Option<Duration>,
    /// Optional client identifier.
    pub identifier: Option<String>,
    /// Sovereign cloud authority host.
    pub authority_host: Option<String>,
    /// Resolved credential block.
    pub credentials: Option<ServiceBusCredentials>,
    /// Sender knobs.
    pub sender: Option<ServiceBusSenderConfig>,
    /// Receiver knobs.
    pub receiver: Option<ServiceBusReceiverConfig>,
    /// Session knobs.
    pub session: Option<ServiceBusSessionConfig>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicy>,
    /// Native DLQ policy.
    pub dlq: Option<DlqPolicy>,
}

/// Errors returned by the Service Bus backend.
#[derive(Debug, Error)]
pub enum ServiceBusQueueError {
    /// The configuration was internally inconsistent.
    #[error("config error: {0}")]
    Config(String),
    /// Failed to validate the credential block.
    #[error("credentials: {0}")]
    Credentials(#[from] ServiceBusCredentialsError),
    /// The Service Bus runtime path is not yet wired in the current build.
    #[error(
        "Service Bus `{operation}` is not yet implemented; the DSL surface is stable but the azservicebus runtime wiring lands in a follow-up release"
    )]
    NotYetImplemented {
        /// Name of the operation the caller invoked.
        operation: &'static str,
    },
    /// `queue()` was called after `close()`.
    #[error("queue is closed")]
    Closed,
}

/// Azure Service Bus queue.
#[derive(Debug, Clone)]
pub struct ServiceBusQueue {
    inner: Arc<ServiceBusQueueInner>,
}

#[derive(Debug)]
struct ServiceBusQueueInner {
    config: ServiceBusQueueConfig,
    closed: AtomicBool,
}

impl ServiceBusQueue {
    /// Construct a new Service Bus queue from a resolved config.
    ///
    /// The current build validates the credential surface eagerly so
    /// users get immediate feedback when they target an as-yet-
    /// unsupported credential variant. Identity / transport mutual
    /// exclusion is also re-checked here as a defence in depth (the
    /// lowerer enforces the same constraints).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceBusQueueError::Config`] when required fields are
    /// missing or inconsistent, and
    /// [`ServiceBusQueueError::Credentials`] when the chosen credential
    /// variant cannot yet be resolved.
    pub fn new(config: ServiceBusQueueConfig) -> Result<Self, ServiceBusQueueError> {
        // Connection-string auth carries the namespace inline; otherwise
        // FQNS is required.
        let needs_fqns = !matches!(
            config.credentials.as_ref(),
            Some(ServiceBusCredentials::ConnectionString { .. })
        );
        if needs_fqns
            && config
                .fully_qualified_namespace
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(ServiceBusQueueError::Config(
                "fully_qualified_namespace is required unless auth.kind = connection_string".into(),
            ));
        }
        match &config.entity {
            ServiceBusEntity::Queue { name } if name.trim().is_empty() => {
                return Err(ServiceBusQueueError::Config(
                    "queue_name must not be empty".into(),
                ));
            }
            ServiceBusEntity::Queue { .. } => {}
            ServiceBusEntity::Subscription {
                topic,
                subscription,
            } => {
                if topic.trim().is_empty() {
                    return Err(ServiceBusQueueError::Config(
                        "topic_name must not be empty for entity_kind = subscription".into(),
                    ));
                }
                if subscription.trim().is_empty() {
                    return Err(ServiceBusQueueError::Config(
                        "subscription_name must not be empty for entity_kind = subscription".into(),
                    ));
                }
            }
        }
        if let Some(transport) = config.transport.as_deref() {
            match transport {
                "amqp_tcp" => {
                    if config.web_proxy.is_some() {
                        return Err(ServiceBusQueueError::Config(
                            "web_proxy requires transport = \"amqp_websockets\"".into(),
                        ));
                    }
                }
                "amqp_websockets" => {}
                other => {
                    return Err(ServiceBusQueueError::Config(format!(
                        "unknown transport `{other}`; expected amqp_tcp | amqp_websockets"
                    )));
                }
            }
        }
        if let Some(creds) = &config.credentials {
            creds.validate()?;
        }
        Ok(Self {
            inner: Arc::new(ServiceBusQueueInner {
                config,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Resolved entity descriptor.
    #[must_use]
    pub fn entity(&self) -> &ServiceBusEntity {
        &self.inner.config.entity
    }
}

impl Queue for ServiceBusQueue {
    type Error = ServiceBusQueueError;

    async fn queue(&self, _signal: Signal, _priority: Priority) -> Result<(), Self::Error> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(ServiceBusQueueError::Closed);
        }
        Err(ServiceBusQueueError::NotYetImplemented { operation: "queue" })
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, Self::Error> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        Err(ServiceBusQueueError::NotYetImplemented {
            operation: "dequeue",
        })
    }

    async fn close(&self) -> Result<(), Self::Error> {
        self.inner.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_config() -> ServiceBusQueueConfig {
        ServiceBusQueueConfig {
            fully_qualified_namespace: Some("iter-bus.servicebus.windows.net".into()),
            entity: ServiceBusEntity::Queue {
                name: "signals".into(),
            },
            transport: None,
            custom_endpoint_address: None,
            web_proxy: None,
            connection_idle_timeout: None,
            identifier: None,
            authority_host: None,
            credentials: Some(ServiceBusCredentials::AadDefault),
            sender: None,
            receiver: None,
            session: None,
            retry: None,
            dlq: None,
        }
    }

    #[test]
    fn new_accepts_minimal_queue_config() {
        let q = ServiceBusQueue::new(queue_config()).expect("queue config");
        assert!(matches!(q.entity(), ServiceBusEntity::Queue { .. }));
    }

    #[test]
    fn new_rejects_missing_fqns_without_connection_string() {
        let mut cfg = queue_config();
        cfg.fully_qualified_namespace = None;
        let err = ServiceBusQueue::new(cfg).expect_err("missing fqns");
        assert!(matches!(err, ServiceBusQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_web_proxy_with_amqp_tcp() {
        let mut cfg = queue_config();
        cfg.transport = Some("amqp_tcp".into());
        cfg.web_proxy = Some(ServiceBusProxyConfig {
            url: "http://proxy.corp:3128".into(),
            username: None,
            password: None,
        });
        let err = ServiceBusQueue::new(cfg).expect_err("proxy + amqp_tcp");
        assert!(matches!(err, ServiceBusQueueError::Config(_)));
    }

    #[test]
    fn new_rejects_unknown_credential_variant() {
        let mut cfg = queue_config();
        cfg.credentials = Some(ServiceBusCredentials::SharedAccessSignature {
            sas_token: "token".into(),
        });
        let err = ServiceBusQueue::new(cfg).expect_err("sas not yet impl");
        assert!(matches!(err, ServiceBusQueueError::Credentials(_)));
    }

    #[tokio::test]
    async fn queue_returns_not_yet_implemented() {
        let q = ServiceBusQueue::new(queue_config()).expect("new");
        let signal = Signal::new(crate::signal::Metadata::new());
        let err = q
            .queue(signal, Priority::default())
            .await
            .expect_err("queue stub errors");
        assert!(matches!(
            err,
            ServiceBusQueueError::NotYetImplemented { operation: "queue" }
        ));
    }

    #[tokio::test]
    async fn dequeue_returns_none_on_cancel() {
        let q = ServiceBusQueue::new(queue_config()).expect("new");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = q.dequeue(cancel).await.expect("cancelled dequeue is Ok");
        assert!(result.is_none());
    }
}
