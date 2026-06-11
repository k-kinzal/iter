//! AWS Kinesis Data Streams `queue kinesis { ... }` AST types.

use super::sqs::{SqsCredentials, SqsHttpClient};
use super::{DlqPolicyDef, MetadataSource, RetryPolicyDef};

/// Top-level `queue kinesis { ... }` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KinesisConfig {
    /// Stream identity. Required.
    pub identity: KinesisIdentity,
    /// Region the stream lives in. Required.
    pub region: Option<String>,
    /// Optional override endpoint (`LocalStack` / Kinesalite).
    pub endpoint_url: Option<String>,
    /// Reuses the AWS credential surface.
    pub credentials: Option<SqsCredentials>,
    /// Reuses the AWS HTTP-client config.
    pub http_client: Option<SqsHttpClient>,
    /// Producer knobs.
    pub producer: Option<KinesisProducer>,
    /// Consumer knobs.
    pub consumer: Option<KinesisConsumer>,
    /// Checkpoint store configuration. Required for stable consumption.
    pub checkpoint: Option<KinesisCheckpoint>,
    /// SDK retry policy.
    pub retry: Option<RetryPolicyDef>,
    /// Iter-implemented DLQ (Kinesis has no native DLQ).
    pub dlq: Option<DlqPolicyDef>,
}

/// Stream identity (ARN preferred over plain name).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KinesisIdentity {
    /// Lowerer placeholder.
    #[default]
    Unset,
    /// Stream ARN — preferred.
    Arn(String),
    /// Plain stream name (looked up against the configured region/account).
    Name(String),
}

/// Producer knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KinesisProducer {
    /// `explicit`, `random` (default), or `from_metadata("k")`.
    pub partition_key_strategy: Option<MetadataSource>,
    /// Per-message explicit hash key escape hatch.
    pub explicit_hash_key: Option<String>,
    /// `none` (default) or `strict_per_key` — auto-chains
    /// `SequenceNumberForOrdering` when strict.
    pub ordering: Option<String>,
    /// `PutRecords` batch size (1–500).
    pub batch_size: Option<i64>,
    /// `PutRecords` batch byte cap (≤ 5 MiB).
    pub batch_max_bytes: Option<i64>,
    /// Linger before flushing a partial batch (seconds).
    pub batch_linger_secs: Option<i64>,
    /// Iter-implemented KPL-style record aggregation.
    pub aggregation: Option<bool>,
}

/// Consumer knobs (a structured block).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KinesisConsumer {
    /// `polling` or `enhanced_fan_out`.
    pub consumer_mode: Option<String>,
    /// Polling iterator type or EFO starting position.
    pub iterator_type: Option<String>,
    /// Required for `AT_/AFTER_SEQUENCE_NUMBER`.
    pub starting_sequence_number: Option<String>,
    /// Required for `AT_TIMESTAMP`.
    pub starting_timestamp: Option<String>,
    /// Polling: max records per `GetRecords` call.
    pub fetch_max_records: Option<i64>,
    /// Polling: poll interval (seconds, ≥ 0.2).
    pub poll_interval_ms: Option<i64>,
    /// EFO: pre-existing consumer ARN.
    pub consumer_arn: Option<String>,
    /// EFO: registered consumer name (alternate to ARN).
    pub consumer_name: Option<String>,
    /// `ListShards` interval (seconds).
    pub shard_discovery_interval_secs: Option<i64>,
    /// Filter discovered shards by id list.
    pub shard_id_filter: Option<Vec<String>>,
    /// Server-side `ShardFilter` block.
    pub shard_list_filter: Option<KinesisShardListFilter>,
}

/// Server-side `ShardFilter` for `ListShards`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KinesisShardListFilter {
    /// Filter type (e.g. `AT_LATEST`, `FROM_TRIM_HORIZON`).
    pub kind: Option<String>,
    /// Optional shard id anchor.
    pub shard_id: Option<String>,
    /// Optional timestamp anchor.
    pub timestamp: Option<String>,
}

/// Checkpoint store configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KinesisCheckpoint {
    /// `dynamodb`, `file`, or `memory`.
    pub store: Option<String>,
    /// `DynamoDB` table name (required when `store = "dynamodb"`).
    pub table_name: Option<String>,
    /// `DynamoDB` region override (defaults to stream region).
    pub region: Option<String>,
    /// `DynamoDB` endpoint override (`LocalStack`).
    pub endpoint_url: Option<String>,
    /// File path (required when `store = "file"`).
    pub path: Option<String>,
    /// Checkpoint flush interval (seconds).
    pub interval_secs: Option<i64>,
    /// Lease duration for multi-worker leasing (seconds).
    pub lease_duration_secs: Option<i64>,
}
