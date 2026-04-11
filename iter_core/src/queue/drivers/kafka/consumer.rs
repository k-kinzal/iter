//! Kafka consumer configuration.

/// Kafka consumer knobs.
#[derive(Debug, Clone, Default)]
pub struct KafkaConsumerConfig {
    /// Topics to subscribe to.
    pub topics: Option<Vec<String>>,
    /// Consumer group id.
    pub group_id: Option<String>,
    /// Static membership id.
    pub group_instance_id: Option<String>,
    /// `earliest`, `latest`, `error`.
    pub auto_offset_reset: Option<String>,
    /// Enable auto offset commit.
    pub enable_auto_commit: Option<bool>,
    /// Auto-commit interval (milliseconds).
    pub auto_commit_interval_ms: Option<u32>,
    /// Enable auto offset store.
    pub enable_auto_offset_store: Option<bool>,
    /// Min fetch bytes per request.
    pub fetch_min_bytes: Option<u32>,
    /// Max fetch bytes per request.
    pub fetch_max_bytes: Option<u32>,
    /// Max fetch bytes per partition.
    pub max_partition_fetch_bytes: Option<u32>,
    /// Wait at most this long when fetching (milliseconds).
    pub fetch_wait_max_ms: Option<u32>,
    /// Backoff between empty fetches (milliseconds).
    pub fetch_queue_backoff_ms: Option<u32>,
    /// Group session timeout (milliseconds).
    pub session_timeout_ms: Option<u32>,
    /// Heartbeat interval (milliseconds).
    pub heartbeat_interval_ms: Option<u32>,
    /// Max poll interval (milliseconds).
    pub max_poll_interval_ms: Option<u32>,
    /// `read_committed` (default) or `read_uncommitted`.
    pub isolation_level: Option<String>,
    /// Group partition assignment strategy.
    pub partition_assignment_strategy: Option<String>,
    /// Verify CRC checksums.
    pub check_crcs: Option<bool>,
    /// Min queued messages.
    pub queued_min_messages: Option<u32>,
    /// Max queued bytes.
    pub queued_max_messages_kbytes: Option<u32>,
    /// Iter-level poll timeout (milliseconds).
    pub poll_timeout_ms: Option<u32>,
}
