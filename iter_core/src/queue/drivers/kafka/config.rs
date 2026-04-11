//! Resolved Kafka queue configuration.

use crate::queue::dlq::DlqPolicy;

use super::consumer::KafkaConsumerConfig;
use super::producer::KafkaProducerConfig;
use super::security::KafkaSecurityConfig;

/// Resolved Kafka queue configuration. Compose-layer responsibility to
/// produce this from the AST (resolving `SecretExpr`, expanding
/// `exactly_once`, etc.) before calling [`KafkaQueue::new`](super::KafkaQueue::new).
#[derive(Debug, Clone)]
pub struct KafkaQueueConfig {
    /// Bootstrap brokers (CSV).
    pub bootstrap_servers: String,
    /// Optional client id.
    pub client_id: Option<String>,
    /// Optional rack id.
    pub client_rack: Option<String>,
    /// `any`, `v4`, or `v6`.
    pub broker_address_family: Option<String>,
    /// Re-resolve broker DNS after this many seconds.
    pub broker_address_ttl_secs: Option<u64>,
    /// Cluster metadata refresh ceiling (seconds).
    pub metadata_max_age_secs: Option<u64>,
    /// Topic metadata refresh interval (seconds).
    pub topic_metadata_refresh_interval_secs: Option<u64>,
    /// Topic metadata refresh fast interval (milliseconds).
    pub topic_metadata_refresh_fast_interval_ms: Option<u64>,
    /// Network socket timeout (seconds).
    pub socket_timeout_secs: Option<u64>,
    /// Enable TCP keepalive.
    pub socket_keepalive_enable: Option<bool>,
    /// Disable Nagle's algorithm.
    pub socket_nagle_disable: Option<bool>,
    /// Connection-failure threshold before broker eviction.
    pub socket_max_fails: Option<u32>,
    /// Reconnect backoff (milliseconds).
    pub reconnect_backoff_ms: Option<u32>,
    /// Reconnect max backoff (milliseconds).
    pub reconnect_backoff_max_ms: Option<u32>,
    /// Send `ApiVersionRequest`.
    pub api_version_request: Option<bool>,
    /// Timeout for the version request (milliseconds).
    pub api_version_request_timeout_ms: Option<u32>,
    /// Security / SASL / TLS settings.
    pub security: Option<KafkaSecurityConfig>,
    /// Producer knobs.
    pub producer: Option<KafkaProducerConfig>,
    /// Consumer knobs.
    pub consumer: Option<KafkaConsumerConfig>,
    /// Expanded `exactly_once` shorthand: when true, the compose layer
    /// has already set `enable_idempotence`, `acks=all`,
    /// `max_in_flight_requests_per_connection=5`, and ensured a
    /// transactional id is present.
    pub exactly_once: bool,
    /// Untyped escape hatch — applied last, overrides any field.
    pub extra_config: Option<Vec<(String, String)>>,
    /// Iter-implemented DLQ.
    pub dlq: Option<DlqPolicy>,
}
