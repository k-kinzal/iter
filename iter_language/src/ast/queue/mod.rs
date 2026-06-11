//! `queue` declaration AST.

pub(super) mod sqs;

pub use sqs::{
    SqsConfig, SqsConsumer, SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity,
    SqsProducer,
};

/// Queue backend declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueDef {
    /// In-process FIFO/priority queue. No fields.
    Memory,
    /// File-backed queue. The `path` field is project-shaped and therefore
    /// required — iter has no honest place to put a queue file on an
    /// arbitrary project.
    File {
        /// File system path where the queue database lives. Required.
        path: String,
    },
    /// Redis-backed queue. Both `url` and `key` are required because the key
    /// namespace is a project-shaped decision (multiple projects sharing a
    /// Redis instance need distinct namespaces).
    Redis {
        /// Redis connection URL.
        url: String,
        /// Redis list key used as the queue namespace.
        key: String,
    },
    /// Shell-driven queue. Users supply `enqueue` and `dequeue` shell command
    /// strings; iter spawns them via `sh -c <command>` (or the configured
    /// `shell`). `enqueue` is one-shot per `queue()` call; `dequeue` is
    /// long-lived and emits NDJSON signal records on stdout.
    ///
    /// This is the escape hatch backend: any queue iter does not ship
    /// first-class can be wrapped here.
    Shell {
        /// Command run for each enqueue. Receives the encoded signal JSON on
        /// stdin and `ITER_SIGNAL_ID`, `ITER_SIGNAL_PRIORITY`,
        /// `ITER_SIGNAL_PRIORITY_NAME` in the environment.
        enqueue: String,
        /// Long-lived dequeue command. Re-spawned on exit until the queue is
        /// closed. Each NDJSON line on stdout produces a signal — either a
        /// full `Signal` object or `{"metadata": {...}, "priority": ...}`
        /// (auto-generated id), mirroring the external trigger format.
        dequeue: String,
        /// Optional cleanup command, run once at queue close.
        close: Option<String>,
        /// Optional interpreter invocation. Defaults to `sh -c`. Must accept
        /// a single argument that is the script to evaluate. Named
        /// `interpreter` rather than `shell` because `shell` is a reserved
        /// keyword used by event-handler actions.
        interpreter: Option<String>,
        /// Per-enqueue timeout in seconds. Defaults to 30s. SIGTERM on
        /// timeout, force-kill afterwards.
        enqueue_timeout_secs: Option<i64>,
    },
    /// AWS Simple Queue Service queue (Standard or FIFO). The
    /// underlying client is built from the credential and HTTP-client
    /// blocks; everything except `region` and the queue identity is
    /// optional and falls through to the AWS default credential chain
    /// when omitted.
    Sqs(Box<SqsConfig>),
}

/// Templated string value: either a literal or a single-argument
/// `from_metadata("key")` call. Applied at send-time per signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataSource {
    /// Static literal value.
    Literal(String),
    /// `from_metadata("key")` — at send time the named metadata field
    /// is read off the signal and substituted.
    FromMetadata(String),
}

/// `RetryPolicy` declaration. Mirrors the runtime retry policy one-for-one;
/// the operator translates it when connecting the queue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetryPolicyDef {
    /// One of `standard` | `adaptive` | `fixed` | `exponential`.
    pub mode: Option<String>,
    /// Max attempts.
    pub max_attempts: Option<i64>,
    /// Initial backoff seconds.
    pub initial_backoff_secs: Option<i64>,
    /// Max backoff seconds.
    pub max_backoff_secs: Option<i64>,
    /// Per-attempt timeout seconds.
    pub try_timeout_secs: Option<i64>,
    /// Optional whitelist of retryable error codes (Pub/Sub-specific
    /// today; harmless on SQS).
    pub retryable_codes: Option<Vec<String>>,
}

/// `DlqPolicy` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DlqPolicyDef {
    /// One of `none` | `native` | `iter_republish`. Defaults to
    /// `none` when the block is absent and to `native` when only a
    /// `kind = "native"` is present.
    pub kind: Option<String>,
    /// Maximum receive count before iter republishes to the DLQ
    /// target (only honoured when `kind = "iter_republish"`).
    pub max_receive_count: Option<i64>,
    /// Optional reason template attached to republished records.
    pub reason_template: Option<String>,
    /// Whether to carry source headers/attributes across.
    pub include_headers: Option<bool>,
    /// Required when `kind = "iter_republish"`: the destination.
    pub target: Option<DlqTargetDef>,
}

/// `DlqTarget` declaration.
///
/// Each variant carries the minimum identity fields its backend
/// needs; richer per-target surfaces (auth blocks, encryption keys,
/// custom endpoints) land alongside the matching backend impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlqTargetDef {
    /// SQS target — republish poison records to another SQS queue.
    Sqs {
        /// Target queue URL.
        queue_url: String,
        /// Region the target queue lives in.
        region: Option<String>,
    },
    /// S3 target — write each poison record as an object.
    S3 {
        /// Bucket name.
        bucket: String,
        /// Optional key prefix.
        prefix: Option<String>,
        /// Region.
        region: Option<String>,
    },
    /// File target — append-only NDJSON, useful for tests and
    /// development.
    File {
        /// Filesystem path.
        path: String,
    },
}
