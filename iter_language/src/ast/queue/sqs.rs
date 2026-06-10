//! AWS SQS-specific `queue sqs { ... }` AST types.

use super::{DlqPolicyDecl, RetryPolicyDecl, TemplatedString};
use crate::ast::SecretExpr;

/// Top-level `queue sqs { ... }` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqsConfig {
    /// Identity of the SQS queue. Either `queue_url` or
    /// (`queue_name` + `account_id`) is required; both forms are
    /// mutually exclusive.
    pub identity: SqsIdentity,
    /// AWS region the queue lives in. Required because the SDK has no
    /// fallback when neither the queue URL nor the credential chain
    /// provides one.
    pub region: Option<String>,
    /// Optional override endpoint (`LocalStack`, VPC endpoints,
    /// FIPS-or-dual-stack-suffixed hosts).
    pub endpoint_url: Option<String>,
    /// Force FIFO mode. Auto-detected from a `.fifo` URL suffix; an
    /// explicit value overrides detection.
    pub fifo: Option<bool>,
    /// Use FIPS-compliant endpoints when supported.
    pub use_fips: Option<bool>,
    /// Use dual-stack (IPv4 + IPv6) endpoints when supported.
    pub use_dual_stack: Option<bool>,
    /// `regional` (default) or `legacy` STS endpoint policy.
    pub sts_regional_endpoints: Option<String>,
    /// Application name propagated into the AWS SDK User-Agent string.
    pub app_name: Option<String>,
    /// Per-field credential layering over the SDK default chain.
    pub credentials: Option<SqsCredentials>,
    /// Connection / request timing knobs.
    pub http_client: Option<SqsHttpClient>,
    /// Producer-side knobs (used when enqueuing).
    pub producer: Option<SqsProducer>,
    /// Consumer-side knobs (used when dequeuing).
    pub consumer: Option<SqsConsumer>,
    /// Retry policy for SDK API calls.
    pub retry: Option<RetryPolicyDecl>,
    /// Dead-letter handling policy.
    pub dlq: Option<DlqPolicyDecl>,
}

/// SQS queue identity. One of `Url` or `NameWithAccount` must be
/// specified; the lowerer rejects both being present at once and both
/// being absent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SqsIdentity {
    /// Lowerer placeholder for "no identity declared yet". Never
    /// reaches consumers — the lowerer surfaces a diagnostic instead.
    #[default]
    Unset,
    /// Full SQS queue URL, e.g. `https://sqs.us-east-1.amazonaws.com/123/q`.
    Url(String),
    /// Queue name plus 12-digit account id; the URL is composed at
    /// build time from the resolved region.
    NameWithAccount {
        /// Queue name (no account-id prefix, no `.fifo` enforcement —
        /// `fifo = true` or the name itself decides).
        name: String,
        /// 12-digit AWS account id.
        account_id: String,
    },
}

/// Composed AWS credential surface. All fields are optional; the
/// resolved credential provider layers each populated field over the
/// SDK default chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqsCredentials {
    /// Credential provider variant to install. Each variant carries
    /// its required + optional sub-fields. Mutually exclusive: the
    /// lowerer rejects mixing per-variant fields under a different
    /// `kind`.
    pub kind: SqsCredentialKind,
}

/// One of the seven supported AWS credential providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqsCredentialKind {
    /// Use the SDK default chain. Equivalent to omitting the entire
    /// `credentials { ... }` block — kept explicit so users can
    /// document intent.
    Default,
    /// Inline access-key triple (dev / `LocalStack`).
    Static {
        /// `AWS_ACCESS_KEY_ID` equivalent.
        access_key_id: SecretExpr,
        /// `AWS_SECRET_ACCESS_KEY` equivalent.
        secret_access_key: SecretExpr,
        /// Optional STS session token.
        session_token: Option<SecretExpr>,
    },
    /// `AssumeRole` over the chain selected by `source_profile` (or
    /// the SDK default chain when absent).
    AssumeRole {
        /// Role ARN to assume. Required.
        role_arn: String,
        /// Optional session name; the SDK will generate one when
        /// absent.
        session_name: Option<String>,
        /// External-id challenge for cross-account roles.
        external_id: Option<SecretExpr>,
        /// Session duration in seconds.
        duration_seconds: Option<i64>,
        /// Profile name whose credentials are used as the source for
        /// the `AssumeRole` call.
        source_profile: Option<String>,
    },
    /// Named profile in `~/.aws/credentials` / `~/.aws/config`.
    Profile {
        /// Profile name.
        name: String,
    },
    /// EKS IRSA / Pod Identity flow. Reads a JWT from `token_file` and
    /// exchanges it for AWS credentials via `AssumeRoleWithWebIdentity`.
    WebIdentityTokenFile {
        /// Role ARN to assume. Required.
        role_arn: String,
        /// Path to the JWT file. Required.
        token_file: String,
        /// Optional session name.
        session_name: Option<String>,
    },
    /// EC2 / ECS instance metadata service.
    Imds,
    /// `credential_process`-style external command.
    Process {
        /// Command line invoked to mint credentials.
        command: String,
    },
}

/// Connection-level HTTP knobs. All fields are optional and pass
/// through to `aws_smithy_http_client::Builder` / hyper config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqsHttpClient {
    /// TCP connect timeout.
    pub connect_timeout_secs: Option<i64>,
    /// Per-read timeout (idle socket).
    pub read_timeout_secs: Option<i64>,
    /// Operation-level timeout: total time, including retries.
    pub operation_timeout_secs: Option<i64>,
    /// Per-attempt timeout: each retry resets it.
    pub operation_attempt_timeout_secs: Option<i64>,
    /// TCP keepalive timer.
    pub tcp_keepalive_secs: Option<i64>,
    /// Pool cap.
    pub max_idle_connections_per_host: Option<i64>,
    /// Pool eviction timer.
    pub connection_pool_idle_timeout_secs: Option<i64>,
    /// HTTP proxy URL.
    pub proxy_url: Option<String>,
    /// `NO_PROXY`-style suffix list.
    pub no_proxy: Option<Vec<String>>,
}

/// Producer-side (`queue` direction) knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqsProducer {
    /// Default per-message `DelaySeconds`. Overridden per-message via
    /// signal metadata is out of scope; a static default is enough for
    /// most rate-limit shapes.
    pub delay_seconds: Option<i64>,
    /// Static `MessageAttributes` overlay applied to every message.
    /// Map of attribute name to scalar string value. Templated values
    /// (e.g. `from_metadata("...")`) are deferred.
    pub message_attributes: Option<Vec<(String, String)>>,
    /// Toggle the `AWSTraceHeader` X-Ray system attribute.
    pub trace_header: Option<bool>,
    /// FIFO `MessageGroupId` source.
    pub message_group_id: Option<TemplatedString>,
    /// FIFO `MessageDeduplicationId` source.
    pub message_deduplication_id: Option<TemplatedString>,
    /// Batch up to this many messages per `SendMessageBatch` call
    /// (1–10 per AWS limit).
    pub batch_size: Option<i64>,
    /// Cap each batch by this many bytes (≤ 262144).
    pub batch_max_bytes: Option<i64>,
    /// Wait at most this duration before flushing a partial batch.
    pub batch_linger_secs: Option<i64>,
}

/// Consumer-side (`dequeue` direction) knobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SqsConsumer {
    /// Hide-time for received messages (seconds).
    pub visibility_timeout_secs: Option<i64>,
    /// Long-poll wait. 0 disables long polling, 20 maximises it.
    pub wait_time_seconds: Option<i64>,
    /// `MaxNumberOfMessages` (1–10).
    pub max_number_of_messages: Option<i64>,
    /// Names of message attributes to fetch.
    pub message_attribute_names: Option<Vec<String>>,
    /// Names of system attributes to fetch (replaces deprecated
    /// `attribute_names`).
    pub message_system_attribute_names: Option<Vec<String>>,
    /// Number of concurrent receive loops to spawn.
    pub concurrent_receivers: Option<i64>,
}
