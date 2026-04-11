//! Apache Kafka `queue kafka { ... }` AST types.

use super::{DlqPolicyDecl, TemplatedString};
use crate::ast::SecretExpr;

/// Top-level `queue kafka { ... }` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KafkaConfig {
    /// Bootstrap brokers (CSV). Required.
    pub bootstrap_servers: String,
    /// Optional client id; iter generates one when omitted.
    pub client_id: Option<String>,
    /// Optional rack id (used for fetch-from-follower).
    pub client_rack: Option<String>,
    /// `any` (default), `v4`, or `v6`.
    pub broker_address_family: Option<String>,
    /// Re-resolve broker DNS after this many seconds.
    pub broker_address_ttl_secs: Option<i64>,
    /// Cluster metadata refresh ceiling (seconds).
    pub metadata_max_age_secs: Option<i64>,
    /// Topic metadata refresh interval (seconds).
    pub topic_metadata_refresh_interval_secs: Option<i64>,
    /// Topic metadata refresh fast interval (milliseconds).
    pub topic_metadata_refresh_fast_interval_ms: Option<i64>,
    /// Network socket timeout (seconds).
    pub socket_timeout_secs: Option<i64>,
    /// Enable TCP keepalive.
    pub socket_keepalive_enable: Option<bool>,
    /// Disable Nagle's algorithm.
    pub socket_nagle_disable: Option<bool>,
    /// Connection-failure threshold before broker eviction.
    pub socket_max_fails: Option<i64>,
    /// Reconnect backoff (milliseconds).
    pub reconnect_backoff_ms: Option<i64>,
    /// Reconnect max backoff (milliseconds).
    pub reconnect_backoff_max_ms: Option<i64>,
    /// Send `ApiVersionRequest` (default true).
    pub api_version_request: Option<bool>,
    /// Timeout for the version request (milliseconds).
    pub api_version_request_timeout_ms: Option<i64>,
    /// Security / SASL / TLS settings.
    pub security: Option<KafkaSecurity>,
    /// Producer knobs (used by [`crate::Queue::queue`]).
    pub producer: Option<KafkaProducer>,
    /// Consumer knobs (used by [`crate::Queue::dequeue`]).
    pub consumer: Option<KafkaConsumer>,
    /// Convenience flag — when true, iter sets idempotence + acks=all
    /// + a transactional id.
    pub exactly_once: Option<bool>,
    /// Untyped escape hatch — applied last, overrides any field.
    pub extra_config: Option<Vec<(String, String)>>,
    /// Iter-implemented DLQ (Kafka has no native DLQ).
    pub dlq: Option<DlqPolicyDecl>,
}

/// Security / SASL / TLS surface.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KafkaSecurity {
    /// `plaintext` (default), `ssl`, `sasl_plaintext`, `sasl_ssl`.
    pub security_protocol: Option<String>,
    /// SASL mechanism.
    pub sasl_mechanism: Option<String>,
    /// SASL/PLAIN/SCRAM username.
    pub sasl_username: Option<SecretExpr>,
    /// SASL/PLAIN/SCRAM password.
    pub sasl_password: Option<SecretExpr>,
    /// Kerberos service name.
    pub sasl_kerberos_service_name: Option<String>,
    /// Kerberos principal.
    pub sasl_kerberos_principal: Option<String>,
    /// Kerberos keytab path.
    pub sasl_kerberos_keytab: Option<String>,
    /// Custom kinit command line.
    pub sasl_kerberos_kinit_cmd: Option<String>,
    /// Min seconds before kinit re-login.
    pub sasl_kerberos_min_time_before_relogin_secs: Option<i64>,
    /// OAUTHBEARER method (`default` or `oidc`).
    pub sasl_oauthbearer_method: Option<String>,
    /// Static OAUTHBEARER config string.
    pub sasl_oauthbearer_config: Option<String>,
    /// OIDC client id.
    pub sasl_oauthbearer_client_id: Option<String>,
    /// OIDC client secret.
    pub sasl_oauthbearer_client_secret: Option<SecretExpr>,
    /// OIDC token endpoint URL.
    pub sasl_oauthbearer_token_endpoint_url: Option<String>,
    /// OIDC scope.
    pub sasl_oauthbearer_scope: Option<String>,
    /// OIDC extensions.
    pub sasl_oauthbearer_extensions: Option<String>,
    /// Allow unsigned JWTs (dev-only).
    pub enable_sasl_oauthbearer_unsecure_jwt: Option<bool>,
    /// SSL CA file path.
    pub ssl_ca_location: Option<String>,
    /// SSL client certificate path.
    pub ssl_certificate_location: Option<String>,
    /// SSL client key path.
    pub ssl_key_location: Option<String>,
    /// SSL client key password.
    pub ssl_key_password: Option<SecretExpr>,
    /// Inline PEM CA bundle.
    pub ssl_ca_pem: Option<SecretExpr>,
    /// Inline PEM client cert.
    pub ssl_certificate_pem: Option<SecretExpr>,
    /// Inline PEM client key.
    pub ssl_key_pem: Option<SecretExpr>,
    /// PKCS12 keystore path.
    pub ssl_keystore_location: Option<String>,
    /// PKCS12 keystore password.
    pub ssl_keystore_password: Option<SecretExpr>,
    /// SSL CRL path.
    pub ssl_crl_location: Option<String>,
    /// Cipher suites.
    pub ssl_cipher_suites: Option<String>,
    /// Allowed elliptic curves.
    pub ssl_curves_list: Option<String>,
    /// Allowed signature algorithms.
    pub ssl_sigalgs_list: Option<String>,
    /// Endpoint identification algorithm (`none` | `https`).
    pub ssl_endpoint_identification_algorithm: Option<String>,
    /// Verify peer certificate (default true).
    pub enable_ssl_certificate_verification: Option<bool>,
    /// HSM engine id.
    pub ssl_engine_id: Option<String>,
    /// HSM engine path.
    pub ssl_engine_location: Option<String>,
}

/// Producer knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KafkaProducer {
    /// Target topic. Required for produce.
    pub topic: Option<String>,
    /// `none`, `leader`, or `all` (default `all`).
    pub acks: Option<String>,
    /// `none`, `gzip`, `snappy`, `lz4`, `zstd`.
    pub compression_type: Option<String>,
    /// Codec-specific compression level.
    pub compression_level: Option<i64>,
    /// Producer batch size in bytes.
    pub batch_size_bytes: Option<i64>,
    /// Producer batch size in messages.
    pub batch_num_messages: Option<i64>,
    /// Linger before flush (milliseconds).
    pub linger_ms: Option<i64>,
    /// Local queue cap in messages.
    pub queue_buffering_max_messages: Option<i64>,
    /// Local queue cap in kilobytes.
    pub queue_buffering_max_kbytes: Option<i64>,
    /// Maximum message size.
    pub message_max_bytes: Option<i64>,
    /// Threshold for zero-copy.
    pub message_copy_max_bytes: Option<i64>,
    /// Max in-flight requests per connection.
    pub max_in_flight_requests_per_connection: Option<i64>,
    /// Per-request timeout (milliseconds).
    pub request_timeout_ms: Option<i64>,
    /// End-to-end message timeout (milliseconds).
    pub message_timeout_ms: Option<i64>,
    /// Total delivery timeout (milliseconds).
    pub delivery_timeout_ms: Option<i64>,
    /// Transactional id (forces idempotence).
    pub transactional_id: Option<String>,
    /// Transaction timeout (milliseconds).
    pub transaction_timeout_ms: Option<i64>,
    /// Enable idempotent producer.
    pub enable_idempotence: Option<bool>,
    /// Maintain gapless guarantee.
    pub enable_gapless_guarantee: Option<bool>,
    /// Partitioner algorithm.
    pub partitioner: Option<String>,
    /// Send retry count (alias `retries`).
    pub message_send_max_retries: Option<i64>,
    /// Retry backoff.
    pub retry_backoff_ms: Option<i64>,
    /// Max retry backoff.
    pub retry_backoff_max_ms: Option<i64>,
    /// Per-message key source.
    pub key_strategy: Option<TemplatedString>,
    /// Static header overlay.
    pub headers: Option<Vec<(String, String)>>,
    /// `signal_created_at` (default) or `now`.
    pub timestamp_strategy: Option<String>,
    /// `partitioner_default` or `from_metadata("k")`.
    pub partition_strategy: Option<TemplatedString>,
}

/// Consumer knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KafkaConsumer {
    /// Topics to subscribe to. Required for dequeue.
    pub topics: Option<Vec<String>>,
    /// Consumer group id. Required.
    pub group_id: Option<String>,
    /// Static membership id.
    pub group_instance_id: Option<String>,
    /// `earliest`, `latest` (default), or `error`.
    pub auto_offset_reset: Option<String>,
    /// Enable auto offset commit (default false; iter commits manually).
    pub enable_auto_commit: Option<bool>,
    /// Auto-commit interval (milliseconds).
    pub auto_commit_interval_ms: Option<i64>,
    /// Enable auto offset store.
    pub enable_auto_offset_store: Option<bool>,
    /// Min fetch bytes per request.
    pub fetch_min_bytes: Option<i64>,
    /// Max fetch bytes per request.
    pub fetch_max_bytes: Option<i64>,
    /// Max fetch bytes per partition.
    pub max_partition_fetch_bytes: Option<i64>,
    /// Wait at most this long when fetching (milliseconds).
    pub fetch_wait_max_ms: Option<i64>,
    /// Backoff between empty fetches (milliseconds).
    pub fetch_queue_backoff_ms: Option<i64>,
    /// Group session timeout (milliseconds).
    pub session_timeout_ms: Option<i64>,
    /// Heartbeat interval (milliseconds).
    pub heartbeat_interval_ms: Option<i64>,
    /// Max poll interval (milliseconds).
    pub max_poll_interval_ms: Option<i64>,
    /// `read_committed` (default) or `read_uncommitted`.
    pub isolation_level: Option<String>,
    /// Group partition assignment strategy.
    pub partition_assignment_strategy: Option<String>,
    /// Verify CRC checksums (default true).
    pub check_crcs: Option<bool>,
    /// Min queued messages.
    pub queued_min_messages: Option<i64>,
    /// Max queued bytes.
    pub queued_max_messages_kbytes: Option<i64>,
    /// Iter-level poll timeout (milliseconds, default 100).
    pub poll_timeout_ms: Option<i64>,
}
