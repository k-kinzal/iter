//! Azure Service Bus `queue servicebus { ... }` AST types.

use super::{DlqPolicyDecl, RetryPolicyDecl, TemplatedString};
use crate::ast::SecretExpr;

/// Top-level `queue servicebus { ... }` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceBusConfig {
    /// Fully-qualified namespace; required unless
    /// `auth.kind = connection_string`.
    pub fully_qualified_namespace: Option<String>,
    /// `queue` or `subscription`.
    pub entity_kind: Option<String>,
    /// Required when `entity_kind = "queue"`.
    pub queue_name: Option<String>,
    /// Required when `entity_kind = "subscription"`.
    pub topic_name: Option<String>,
    /// Required when `entity_kind = "subscription"`.
    pub subscription_name: Option<String>,
    /// `amqp_tcp` (default) or `amqp_websockets`.
    pub transport: Option<String>,
    /// Optional private-endpoint host.
    pub custom_endpoint_address: Option<String>,
    /// WebSocket proxy (only valid with `transport = "amqp_websockets"`).
    pub web_proxy: Option<ServiceBusProxy>,
    /// Connection idle timeout (seconds).
    pub connection_idle_timeout_secs: Option<i64>,
    /// Optional client identifier.
    pub identifier: Option<String>,
    /// Sovereign cloud authority host.
    pub authority_host: Option<String>,
    /// Auth surface.
    pub auth: Option<ServiceBusAuth>,
    /// Sender knobs.
    pub sender: Option<ServiceBusSender>,
    /// Receiver knobs.
    pub receiver: Option<ServiceBusReceiver>,
    /// Session knobs (required when entity has `RequiresSession = true`).
    pub session: Option<ServiceBusSession>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicyDecl>,
    /// Native DLQ — typically observed via `sub_queue = dead_letter`.
    pub dlq: Option<DlqPolicyDecl>,
}

/// WebSocket proxy configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceBusProxy {
    /// Proxy URL.
    pub url: String,
    /// Optional proxy username.
    pub username: Option<String>,
    /// Optional proxy password.
    pub password: Option<SecretExpr>,
}

/// Auth surface (`auth.kind` is variant-tagged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceBusAuth {
    /// Selected auth variant.
    pub kind: ServiceBusAuthKind,
}

/// Supported Service Bus auth providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceBusAuthKind {
    /// Native chain (Managed Identity → Workload Identity → Az CLI).
    AadDefault,
    /// SAS connection string.
    ConnectionString {
        /// Full Service Bus connection string.
        value: SecretExpr,
    },
    /// Pre-signed SAS token.
    SharedAccessSignature {
        /// SAS token string.
        sas_token: SecretExpr,
    },
    /// AAD client-secret credential.
    AadClientSecret {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client (application) id (UUID).
        client_id: String,
        /// Client secret.
        client_secret: SecretExpr,
    },
    /// AAD client-certificate credential.
    AadClientCertificate {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client (application) id (UUID).
        client_id: String,
        /// Path to a PEM/PFX certificate.
        cert_path: String,
        /// Optional certificate password.
        cert_password: Option<SecretExpr>,
    },
    /// Managed Identity (system-assigned when `client_id` omitted).
    AadManagedIdentity {
        /// Optional user-assigned identity id.
        client_id: Option<String>,
    },
    /// Workload Identity (AKS).
    AadWorkloadIdentity {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client id (UUID).
        client_id: String,
        /// Federated token file path.
        token_file: String,
    },
}

/// Sender knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceBusSender {
    /// Per-message id template.
    pub message_id: Option<TemplatedString>,
    /// Correlation id template.
    pub correlation_id: Option<TemplatedString>,
    /// Static content type.
    pub content_type: Option<String>,
    /// Static subject.
    pub subject: Option<String>,
    /// Reply-to entity name.
    pub reply_to: Option<String>,
    /// Reply-to session id.
    pub reply_to_session_id: Option<String>,
    /// Per-message TTL (seconds).
    pub time_to_live_secs: Option<i64>,
    /// RFC3339 scheduled enqueue time.
    pub scheduled_enqueue_time: Option<String>,
    /// `none` or `from_metadata("k")`.
    pub partition_key_strategy: Option<TemplatedString>,
    /// `none` or `from_metadata("k")` (used for sessions).
    pub session_id_strategy: Option<TemplatedString>,
    /// Static application-property overlay.
    pub application_properties: Option<Vec<(String, String)>>,
    /// Batch size cap.
    pub batch_size: Option<i64>,
    /// Batch byte cap (Standard 256 KB / Premium 1 MB).
    pub batch_max_bytes: Option<i64>,
    /// Linger before flushing a partial batch (seconds).
    pub batch_linger_secs: Option<i64>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicyDecl>,
}

/// Receiver knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceBusReceiver {
    /// `peek_lock` (default) or `receive_and_delete`.
    pub receive_mode: Option<String>,
    /// Prefetch count.
    pub prefetch_count: Option<i64>,
    /// `none` (default), `dead_letter`, or `transfer_dead_letter`.
    pub sub_queue: Option<String>,
    /// Optional client identifier.
    pub identifier: Option<String>,
    /// Max wait per receive batch (seconds).
    pub max_wait_time_secs: Option<i64>,
    /// Max messages per receive batch.
    pub max_messages: Option<i64>,
    /// Max auto lock-renewal duration (seconds).
    pub max_auto_lock_renewal_duration_secs: Option<i64>,
    /// `abandon` (default), `dead_letter`, or `defer`.
    pub on_handler_error: Option<String>,
    /// DLQ reason template (`{{error.kind}}` etc.).
    pub dead_letter_reason_template: Option<String>,
    /// DLQ description template.
    pub dead_letter_description_template: Option<String>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicyDecl>,
}

/// Session knobs (required when entity has `RequiresSession = true`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceBusSession {
    /// `accept_specific` or `accept_next`.
    pub mode: Option<String>,
    /// Required when `mode = "accept_specific"`.
    pub session_id: Option<String>,
    /// Idle timeout before releasing the session (seconds).
    pub session_idle_timeout_secs: Option<i64>,
}
