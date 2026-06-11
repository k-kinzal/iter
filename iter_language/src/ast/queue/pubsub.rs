//! GCP Pub/Sub `queue pubsub { ... }` AST types.

use super::{DlqPolicyDef, MetadataSource, RetryPolicyDef};
use crate::ast::SecretExpr;

/// Top-level `queue pubsub { ... }` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PubSubConfig {
    /// GCP project hosting the topic and subscription. Required.
    pub project: String,
    /// Topic id used when enqueuing. Required.
    pub topic: String,
    /// Subscription id used when dequeuing. Required.
    pub subscription: String,
    /// Optional regional endpoint or emulator host (`PUBSUB_EMULATOR_HOST`).
    pub endpoint: Option<String>,
    /// Optional User-Agent override.
    pub user_agent: Option<String>,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: Option<i64>,
    /// Per-request timeout in seconds.
    pub request_timeout_secs: Option<i64>,
    /// gRPC channel keepalive knobs.
    pub keepalive: Option<PubSubKeepalive>,
    /// Quota project to bill API calls against.
    pub quota_project: Option<String>,
    /// OAuth scopes; defaults to the `pubsub` scope when absent.
    pub scopes: Option<Vec<String>>,
    /// Per-field credential layering over ADC.
    pub credentials: Option<PubSubCredentials>,
    /// Producer (publisher) knobs.
    pub publisher: Option<PubSubPublisher>,
    /// Consumer (subscriber) knobs.
    pub subscriber: Option<PubSubSubscriber>,
    /// Optional idempotent startup seek operation.
    pub initial_seek: Option<PubSubInitialSeek>,
    /// Dead-letter handling — typically `Native` (configured outside iter).
    pub dlq: Option<DlqPolicyDef>,
}

/// gRPC channel keepalive parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PubSubKeepalive {
    /// Idle time before a keepalive ping (seconds).
    pub time_secs: Option<i64>,
    /// Ack-deadline for the keepalive ping (seconds).
    pub timeout_secs: Option<i64>,
    /// Allow keepalive pings on idle channels.
    pub permit_without_stream: Option<bool>,
}

/// Composed GCP credential surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubSubCredentials {
    /// Selected credential variant.
    pub kind: PubSubCredentialKind,
}

/// Supported GCP credential providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PubSubCredentialKind {
    /// Application Default Credentials chain. Equivalent to omitting
    /// the credentials block; kept explicit so users can document
    /// intent.
    Adc,
    /// Service-account JSON loaded from a path on disk.
    ServiceAccountFile {
        /// Filesystem path to the JSON key file.
        path: String,
    },
    /// Service-account JSON supplied inline (or via `env(...)`).
    ServiceAccountInline {
        /// JSON document — typically `env("GCP_SERVICE_ACCOUNT_JSON")`.
        json: SecretExpr,
    },
    /// External-account / Workload Identity Federation flow.
    WorkloadIdentity {
        /// Audience the `IdP` token is minted for.
        audience: String,
        /// Filesystem path to the `IdP` token file.
        token_file: String,
        /// Optional service-account to impersonate after federation.
        impersonation_target: Option<String>,
    },
    /// SA impersonation chain.
    Impersonate {
        /// Final principal to impersonate.
        target_principal: String,
        /// Optional intermediate principals.
        delegates: Option<Vec<String>>,
        /// OAuth scopes.
        scopes: Option<Vec<String>>,
    },
    /// Pre-minted access token (testing / niche flows).
    AccessToken {
        /// Bearer token; typically `env("...")`.
        token: SecretExpr,
        /// RFC3339 expiry timestamp (optional).
        expiry: Option<String>,
    },
}

/// Producer-side knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PubSubPublisher {
    /// Batch flush delay in milliseconds.
    pub delay_threshold_ms: Option<i64>,
    /// Flush after this many messages.
    pub count_threshold: Option<i64>,
    /// Flush after this many bytes.
    pub byte_threshold: Option<i64>,
    /// Backpressure cap on outstanding messages.
    pub max_outstanding_messages: Option<i64>,
    /// Backpressure cap on outstanding bytes.
    pub max_outstanding_bytes: Option<i64>,
    /// `block` (default) or `error` when the cap is hit.
    pub limit_exceeded_behavior: Option<String>,
    /// Worker thread count.
    pub workers: Option<i64>,
    /// Per-publish RPC timeout in seconds.
    pub request_timeout_secs: Option<i64>,
    /// SDK retry policy for publish RPCs.
    pub retry: Option<RetryPolicyDef>,
    /// Enable gRPC compression.
    pub enable_compression: Option<bool>,
    /// Minimum payload size to compress.
    pub compression_bytes_threshold: Option<i64>,
    /// Static attribute overlay applied to every message.
    pub attributes: Option<Vec<(String, String)>>,
    /// Ordering key source (`none` or `from_metadata("k")`).
    pub ordering_key_strategy: Option<MetadataSource>,
}

/// Consumer-side knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PubSubSubscriber {
    /// `streaming` (default) or `sync` pull mode.
    pub pull_mode: Option<String>,
    /// Streaming-only: extended ack deadline (10–600s, default 60).
    pub stream_ack_deadline_seconds: Option<i64>,
    /// Streaming-only: backpressure cap on outstanding messages.
    pub max_outstanding_messages: Option<i64>,
    /// Streaming-only: backpressure cap on outstanding bytes.
    pub max_outstanding_bytes: Option<i64>,
    /// Streaming-only: minimum lease-extension interval (seconds).
    pub min_duration_per_lease_extension_secs: Option<i64>,
    /// Streaming-only: maximum lease-extension interval (seconds).
    pub max_duration_per_lease_extension_secs: Option<i64>,
    /// Streaming-only: keepalive ping interval (seconds).
    pub ping_interval_secs: Option<i64>,
    /// Sync-only: max messages per pull call (≤ 1000).
    pub max_messages: Option<i64>,
    /// Sync-only: return immediately on empty pull (default false).
    pub return_immediately: Option<bool>,
    /// Retry policy for receive RPCs.
    pub retry: Option<RetryPolicyDef>,
}

/// Idempotent startup seek operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubSubInitialSeek {
    /// `timestamp` or `snapshot`.
    pub kind: String,
    /// RFC3339 timestamp when `kind = "timestamp"`.
    pub timestamp: Option<String>,
    /// Snapshot name when `kind = "snapshot"`.
    pub snapshot_name: Option<String>,
}
