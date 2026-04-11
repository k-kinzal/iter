//! Kafka producer configuration.

/// Kafka producer knobs.
#[derive(Debug, Clone, Default)]
pub struct KafkaProducerConfig {
    /// Target topic.
    pub topic: Option<String>,
    /// `none`, `leader`, `all`.
    pub acks: Option<String>,
    /// `none`, `gzip`, `snappy`, `lz4`, `zstd`.
    pub compression_type: Option<String>,
    /// Codec-specific compression level.
    pub compression_level: Option<i32>,
    /// Producer batch size in bytes.
    pub batch_size_bytes: Option<u32>,
    /// Producer batch size in messages.
    pub batch_num_messages: Option<u32>,
    /// Linger before flush (milliseconds).
    pub linger_ms: Option<u32>,
    /// Local queue cap in messages.
    pub queue_buffering_max_messages: Option<u32>,
    /// Local queue cap in kilobytes.
    pub queue_buffering_max_kbytes: Option<u32>,
    /// Maximum message size.
    pub message_max_bytes: Option<u32>,
    /// Threshold for zero-copy.
    pub message_copy_max_bytes: Option<u32>,
    /// Max in-flight requests per connection.
    pub max_in_flight_requests_per_connection: Option<u32>,
    /// Per-request timeout (milliseconds).
    pub request_timeout_ms: Option<u32>,
    /// End-to-end message timeout (milliseconds).
    pub message_timeout_ms: Option<u32>,
    /// Total delivery timeout (milliseconds).
    pub delivery_timeout_ms: Option<u32>,
    /// Transactional id (forces idempotence).
    pub transactional_id: Option<String>,
    /// Transaction timeout (milliseconds).
    pub transaction_timeout_ms: Option<u32>,
    /// Enable idempotent producer.
    pub enable_idempotence: Option<bool>,
    /// Maintain gapless guarantee.
    pub enable_gapless_guarantee: Option<bool>,
    /// Partitioner algorithm.
    pub partitioner: Option<String>,
    /// Send retry count.
    pub message_send_max_retries: Option<u32>,
    /// Retry backoff.
    pub retry_backoff_ms: Option<u32>,
    /// Max retry backoff.
    pub retry_backoff_max_ms: Option<u32>,
    /// Per-message key source. `None` means no key.
    pub key_strategy_metadata: Option<String>,
    /// Use `signal_id` as the message key when `key_strategy_metadata`
    /// is unset.
    pub key_from_signal_id: bool,
    /// Static header overlay.
    pub headers: Option<Vec<(String, String)>>,
    /// `signal_created_at` (default) or `now`.
    pub timestamp_strategy: Option<String>,
    /// Per-message partition source.
    pub partition_strategy_metadata: Option<String>,
}
