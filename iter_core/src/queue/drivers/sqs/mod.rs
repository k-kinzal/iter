//! Amazon SQS queue backend.
//!
//! Standard and FIFO queues are supported through the same
//! [`SqsQueue`] type. The backend long-polls `ReceiveMessage` inside a
//! `tokio::select!` for cancel-aware dequeue, then issues
//! `DeleteMessage` immediately on receipt to preserve the at-most-once
//! delivery contract documented on [`crate::Queue`].
//!
//! The wire format is the shared
//! [`encode_signal`](crate::queue::encode_signal) /
//! [`decode_signal`](crate::queue::decode_signal) envelope so SQS messages
//! interleave cleanly with other backends. Priority is also projected
//! out-of-band as a `String`-typed message attribute named
//! `iter.priority` so SQS console / `CloudWatch` logs surface it directly.
//!
//! Standard SQS queues do not honour priority for delivery ordering — see
//! the trait docs on [`crate::Queue`] for the "best-effort priority"
//! contract that applies here.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use aws_config::{BehaviorVersion, Region};
use aws_sdk_sqs::{
    Client,
    error::SdkError,
    types::{MessageAttributeValue, MessageSystemAttributeName},
};
use aws_smithy_types::retry::{RetryConfig, RetryMode as SdkRetryMode};
use aws_types::{SdkConfig, app_name::AppName};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::queue::dlq::DlqPolicy;
use crate::queue::{MetadataSource, MissingMetadata, Priority, Queue, QueueError};
use async_trait::async_trait;
use crate::queue::drivers::aws::credentials::{
    AwsCredentials, CredentialsBuildError, build_credentials,
};
use crate::queue::drivers::aws::http::{AwsHttpClientConfig, build_http_client};
use crate::queue::envelope::{EnvelopeError, decode_signal, encode_signal};
use crate::queue::retry::{RetryMode, RetryPolicy};
use crate::signal::Signal;

/// Identity for the SQS queue. Either a fully-qualified URL or a
/// `(queue_name, account_id)` pair from which the URL is derived using
/// the configured region/endpoint.
#[derive(Debug, Clone)]
pub enum SqsIdentity {
    /// Fully-qualified queue URL (e.g.
    /// `https://sqs.us-east-1.amazonaws.com/123456789012/iter`).
    Url(String),
    /// `queue_name` + `account_id`. The URL is constructed at queue
    /// build time using the resolved region and `endpoint_url`.
    NameWithAccount {
        /// SQS queue name.
        name: String,
        /// 12-digit AWS account ID owning the queue.
        account_id: String,
    },
}

/// Producer-side knobs for both standard and FIFO queues.
#[derive(Debug, Clone, Default)]
pub struct SqsProducerConfig {
    /// Per-message default delay (standard queues only).
    pub delay_seconds: Option<u32>,
    /// Static `MessageAttributes` attached to every send.
    pub message_attributes: Option<Vec<(String, String)>>,
    /// Toggle the X-Ray `AWSTraceHeader` system attribute. Currently
    /// informational; iter does not propagate a trace context yet.
    pub trace_header: Option<bool>,
    /// FIFO `MessageGroupId` source. Required for FIFO queues.
    pub message_group_id: Option<MetadataSource>,
    /// FIFO `MessageDeduplicationId` source.
    pub message_deduplication_id: Option<MetadataSource>,
    /// `SendMessageBatch` size (1–10). Currently informational; this
    /// implementation issues one `SendMessage` per `queue()` call.
    pub batch_size: Option<u32>,
    /// `SendMessageBatch` payload ceiling (≤`262_144`).
    pub batch_max_bytes: Option<u32>,
    /// Maximum buffering wait before flushing a partial batch.
    pub batch_linger: Option<Duration>,
}

/// Consumer-side knobs.
#[derive(Debug, Clone, Default)]
pub struct SqsConsumerConfig {
    /// `VisibilityTimeout` per receive (`0–43_200s`).
    pub visibility_timeout: Option<Duration>,
    /// Long-poll wait time (0–20s). Defaults to 20.
    pub wait_time_seconds: Option<u32>,
    /// `MaxNumberOfMessages` per `ReceiveMessage` (1–10). Defaults to 1.
    pub max_number_of_messages: Option<u32>,
    /// Custom `MessageAttribute` names to request.
    pub message_attribute_names: Option<Vec<String>>,
    /// Custom `MessageSystemAttribute` names to request.
    pub message_system_attribute_names: Option<Vec<String>>,
    /// Number of parallel receive loops to run. Currently informational;
    /// this implementation runs a single receiver.
    pub concurrent_receivers: Option<u32>,
}

/// Resolved SQS queue configuration. Compose-layer responsibility to
/// produce this from the AST (resolving `SecretExpr`, etc.) before
/// calling [`SqsQueue::new`].
#[derive(Debug, Clone)]
pub struct SqsQueueConfig {
    /// Queue identity (URL or name+account).
    pub identity: SqsIdentity,
    /// Service region. Required when `identity` is `NameWithAccount`;
    /// otherwise inferred from the URL.
    pub region: Option<String>,
    /// Custom endpoint URL (`LocalStack` / VPC endpoints).
    pub endpoint_url: Option<String>,
    /// Override FIFO mode detection. When `None`, FIFO is auto-detected
    /// from a `.fifo` suffix on the resolved URL.
    pub fifo: Option<bool>,
    /// Force FIPS endpoints.
    pub use_fips: Option<bool>,
    /// Force dual-stack (IPv6) endpoints.
    pub use_dual_stack: Option<bool>,
    /// `regional` / `legacy` STS endpoint mode. Currently informational.
    pub sts_regional_endpoints: Option<String>,
    /// Application name forwarded into the User-Agent.
    pub app_name: Option<String>,
    /// Resolved credential block.
    pub credentials: Option<AwsCredentials>,
    /// Resolved HTTP-client tuning.
    pub http_client: Option<AwsHttpClientConfig>,
    /// Producer-side knobs.
    pub producer: Option<SqsProducerConfig>,
    /// Consumer-side knobs.
    pub consumer: Option<SqsConsumerConfig>,
    /// Retry policy applied to send / receive operations.
    pub retry: Option<RetryPolicy>,
    /// DLQ policy. SQS uses native DLQs configured on the queue itself;
    /// `IterRepublish` is only used when iter is asked to republish into
    /// a separate target. Currently informational here — the runner
    /// observes native DLQs out-of-band.
    pub dlq: Option<DlqPolicy>,
}

/// Errors returned by the SQS backend.
#[derive(Debug, Error)]
pub enum SqsQueueError {
    /// The configuration was internally inconsistent (e.g. missing
    /// region for a `NameWithAccount` identity).
    #[error("config error: {0}")]
    Config(String),
    /// Failed to build the credential provider.
    #[error("credentials: {0}")]
    Credentials(#[from] CredentialsBuildError),
    /// `SendMessage` call failed.
    #[error("send_message failed: {0}")]
    SendMessage(String),
    /// `ReceiveMessage` call failed.
    #[error("receive_message failed: {0}")]
    ReceiveMessage(String),
    /// `DeleteMessage` call failed.
    #[error("delete_message failed: {0}")]
    DeleteMessage(String),
    /// A message body could not be decoded as a versioned envelope.
    #[error("decode envelope: {0}")]
    Envelope(#[from] EnvelopeError),
    /// A message arrived without a body.
    #[error("received SQS message without a body (message_id={message_id:?})")]
    EmptyBody {
        /// SQS-assigned id, when present.
        message_id: Option<String>,
    },
    /// A FIFO send template referenced a metadata key that the signal
    /// did not carry.
    #[error("FIFO template references metadata key `{key}` that the signal does not have")]
    MissingTemplateMetadata {
        /// Metadata key that was missing.
        key: String,
    },
    /// The caller supplied a FIFO queue with no `message_group_id`
    /// template; SQS rejects FIFO sends without one.
    #[error("FIFO queue requires producer.message_group_id; none configured")]
    FifoMissingGroupId,
    /// `enqueue()` was called after `close()`.
    #[error("queue is closed")]
    Closed,
}

impl From<MissingMetadata> for SqsQueueError {
    fn from(err: MissingMetadata) -> Self {
        Self::MissingTemplateMetadata { key: err.key }
    }
}

/// Default long-poll wait when the consumer block is omitted.
const DEFAULT_WAIT_TIME_SECS: u32 = 20;

/// Custom message-attribute name used to surface the iter priority on
/// the SQS console.
const PRIORITY_ATTR: &str = "iter.priority";

/// Amazon SQS queue.
#[derive(Debug, Clone)]
pub struct SqsQueue {
    inner: Arc<SqsQueueInner>,
}

#[derive(Debug)]
struct SqsQueueInner {
    client: Client,
    queue_url: String,
    fifo: bool,
    producer: SqsProducerConfig,
    consumer: SqsConsumerConfig,
    closed: AtomicBool,
}

impl SqsQueue {
    /// Construct a new SQS queue from a resolved config and connect to
    /// the service.
    ///
    /// # Errors
    ///
    /// Returns [`SqsQueueError::Config`] when the identity / region pair
    /// is inconsistent, [`SqsQueueError::Credentials`] when the
    /// credential provider cannot be built, and
    /// [`SqsQueueError::HttpClient`] when the HTTP client / timeout
    /// artifacts fail to build.
    pub async fn new(config: SqsQueueConfig) -> Result<Self, SqsQueueError> {
        let queue_url = resolve_queue_url(&config)?;
        let fifo = config
            .fifo
            .unwrap_or_else(|| queue_url.as_bytes().ends_with(b".fifo"));

        let mut sdk_builder = SdkConfig::builder().behavior_version(BehaviorVersion::latest());
        if let Some(region) = config.region.clone() {
            sdk_builder = sdk_builder.region(Region::new(region));
        } else if let Some(region) = region_from_url(&queue_url) {
            sdk_builder = sdk_builder.region(Region::new(region));
        }
        if let Some(endpoint) = config.endpoint_url.clone() {
            sdk_builder = sdk_builder.endpoint_url(endpoint);
        }
        if let Some(use_fips) = config.use_fips {
            sdk_builder = sdk_builder.use_fips(use_fips);
        }
        if let Some(use_dual_stack) = config.use_dual_stack {
            sdk_builder = sdk_builder.use_dual_stack(use_dual_stack);
        }
        if let Some(app_name) = config.app_name.clone() {
            match AppName::new(app_name.clone()) {
                Ok(an) => sdk_builder = sdk_builder.app_name(an),
                Err(e) => {
                    return Err(SqsQueueError::Config(format!(
                        "invalid app_name `{app_name}`: {e}"
                    )));
                }
            }
        }

        let creds = config
            .credentials
            .clone()
            .unwrap_or(AwsCredentials::Default);
        let provider = build_credentials(&creds).await?;
        sdk_builder = sdk_builder.credentials_provider(provider);

        let http = config.http_client.clone().unwrap_or_default();
        let artifacts = build_http_client(&http);
        if let Some(client) = artifacts.http_client {
            sdk_builder = sdk_builder.http_client(client);
        }
        if let Some(timeout) = artifacts.timeout_config {
            sdk_builder = sdk_builder.timeout_config(timeout);
        }

        if let Some(retry) = &config.retry {
            sdk_builder = sdk_builder.retry_config(translate_retry(retry));
        }

        let client = Client::new(&sdk_builder.build());

        Ok(Self {
            inner: Arc::new(SqsQueueInner {
                client,
                queue_url,
                fifo,
                producer: config.producer.unwrap_or_default(),
                consumer: config.consumer.unwrap_or_default(),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Resolved queue URL after applying identity + region.
    #[must_use]
    pub fn queue_url(&self) -> &str {
        &self.inner.queue_url
    }

    /// `true` when the resolved queue is a FIFO queue.
    #[must_use]
    pub fn is_fifo(&self) -> bool {
        self.inner.fifo
    }
}

fn resolve_queue_url(config: &SqsQueueConfig) -> Result<String, SqsQueueError> {
    match &config.identity {
        SqsIdentity::Url(url) => Ok(url.clone()),
        SqsIdentity::NameWithAccount { name, account_id } => {
            let region = config.region.as_deref().ok_or_else(|| {
                SqsQueueError::Config(
                    "queue_name + account_id requires `region` to construct the queue URL".into(),
                )
            })?;
            // Honour endpoint_url for LocalStack-style overrides.
            let host = config
                .endpoint_url
                .clone()
                .unwrap_or_else(|| format!("https://sqs.{region}.amazonaws.com"));
            let host = host.trim_end_matches('/').to_string();
            Ok(format!("{host}/{account_id}/{name}"))
        }
    }
}

/// Best-effort region extraction from the SQS hostname pattern
/// `sqs.<region>.amazonaws.com`. Returns `None` for non-canonical hosts
/// (`LocalStack`, VPC endpoints) — the caller falls back to whatever the
/// SDK default chain resolves.
fn region_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let host = after_scheme.split('/').next()?;
    let mut parts = host.split('.');
    if parts.next()? != "sqs" {
        return None;
    }
    let region = parts.next()?;
    if parts.next()? == "amazonaws" {
        Some(region.to_string())
    } else {
        None
    }
}

fn translate_retry(policy: &RetryPolicy) -> RetryConfig {
    let mode = match policy.mode {
        RetryMode::Standard | RetryMode::Fixed | RetryMode::Exponential => SdkRetryMode::Standard,
        RetryMode::Adaptive => SdkRetryMode::Adaptive,
    };
    RetryConfig::standard()
        .with_retry_mode(mode)
        .with_max_attempts(policy.max_attempts)
        .with_initial_backoff(policy.initial_backoff)
        .with_max_backoff(policy.max_backoff)
}

impl SqsQueue {
    async fn enqueue_signal(
        &self,
        signal: Signal,
        priority: Priority,
    ) -> Result<(), SqsQueueError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(SqsQueueError::Closed);
        }

        let body = encode_signal(&signal, priority);
        let body_str = String::from_utf8(body)
            .expect("encode_signal returns valid UTF-8 JSON; this conversion cannot fail");

        let mut request = self
            .inner
            .client
            .send_message()
            .queue_url(&self.inner.queue_url)
            .message_body(body_str);

        // Always project the priority on its own attribute so SQS
        // console / CloudWatch logs surface it.
        let priority_attr = MessageAttributeValue::builder()
            .data_type("String")
            .string_value(priority.value().to_string())
            .build()
            .map_err(|e| SqsQueueError::SendMessage(format!("priority attribute: {e}")))?;
        request = request.message_attributes(PRIORITY_ATTR, priority_attr);

        if let Some(statics) = &self.inner.producer.message_attributes {
            for (k, v) in statics {
                let attr = MessageAttributeValue::builder()
                    .data_type("String")
                    .string_value(v.clone())
                    .build()
                    .map_err(|e| {
                        SqsQueueError::SendMessage(format!("message_attribute `{k}`: {e}"))
                    })?;
                request = request.message_attributes(k, attr);
            }
        }

        if let Some(delay) = self.inner.producer.delay_seconds {
            request = request.delay_seconds(i32::try_from(delay).unwrap_or(i32::MAX));
        }

        if self.inner.fifo {
            let group = self
                .inner
                .producer
                .message_group_id
                .as_ref()
                .ok_or(SqsQueueError::FifoMissingGroupId)?
                .resolve(&signal)?;
            request = request.message_group_id(group);

            if let Some(dedup) = &self.inner.producer.message_deduplication_id {
                request = request.message_deduplication_id(dedup.resolve(&signal)?);
            }
        }

        request
            .send()
            .await
            .map_err(|e| SqsQueueError::SendMessage(format_sdk_error(&e)))?;
        Ok(())
    }

    async fn dequeue_signal(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Signal>, SqsQueueError> {
        loop {
            if cancel.is_cancelled() || self.inner.closed.load(Ordering::Acquire) {
                return Ok(None);
            }

            let wait = self
                .inner
                .consumer
                .wait_time_seconds
                .unwrap_or(DEFAULT_WAIT_TIME_SECS);
            let max_messages = self.inner.consumer.max_number_of_messages.unwrap_or(1);

            let mut request = self
                .inner
                .client
                .receive_message()
                .queue_url(&self.inner.queue_url)
                .wait_time_seconds(i32::try_from(wait).unwrap_or(i32::MAX))
                .max_number_of_messages(i32::try_from(max_messages).unwrap_or(i32::MAX));

            if let Some(vt) = self.inner.consumer.visibility_timeout {
                request =
                    request.visibility_timeout(i32::try_from(vt.as_secs()).unwrap_or(i32::MAX));
            }
            if let Some(names) = &self.inner.consumer.message_attribute_names {
                for name in names {
                    request = request.message_attribute_names(name);
                }
            }
            if let Some(names) = &self.inner.consumer.message_system_attribute_names {
                for name in names {
                    if let Ok(parsed) = MessageSystemAttributeName::try_parse(name.as_str()) {
                        request = request.message_system_attribute_names(parsed);
                    } else {
                        tracing::warn!("ignoring unrecognised SQS system attribute name `{name}`");
                    }
                }
            }

            let receive = request.send();

            let response = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                r = receive => r,
            };

            let response =
                response.map_err(|e| SqsQueueError::ReceiveMessage(format_sdk_error(&e)))?;

            let messages = response.messages.unwrap_or_default();
            if messages.is_empty() {
                // Long-poll timed out with nothing to deliver — re-check
                // cancel / closed flags, then poll again.
                continue;
            }

            // We requested `max_messages` but only return one per
            // dequeue call. Process in order, deleting the chosen
            // message immediately and abandoning the rest (their
            // visibility timeouts will expire and they'll be redelivered
            // on the next call). This keeps the trait shape simple.
            //
            // For configs with `max_number_of_messages = 1` this is
            // exact. For configs that pre-fetch >1 we lose throughput
            // but stay correct. A bounded buffer is a future
            // optimisation tracked alongside the queue plan.
            for message in messages {
                let Some(receipt) = message.receipt_handle.clone() else {
                    tracing::warn!(
                        message_id = ?message.message_id,
                        "SQS message arrived without a receipt_handle; cannot ack — skipping"
                    );
                    continue;
                };

                let Some(body) = message.body.as_deref() else {
                    // Delete the empty message so it doesn't get
                    // redelivered forever.
                    drop(
                        self.inner
                            .client
                            .delete_message()
                            .queue_url(&self.inner.queue_url)
                            .receipt_handle(&receipt)
                            .send()
                            .await,
                    );
                    return Err(SqsQueueError::EmptyBody {
                        message_id: message.message_id,
                    });
                };

                let (signal, _priority) = decode_signal(body.as_bytes())?;

                self.inner
                    .client
                    .delete_message()
                    .queue_url(&self.inner.queue_url)
                    .receipt_handle(&receipt)
                    .send()
                    .await
                    .map_err(|e| SqsQueueError::DeleteMessage(format_sdk_error(&e)))?;

                return Ok(Some(signal));
            }
        }
    }

}

#[async_trait]
impl Queue for SqsQueue {
    async fn enqueue(&self, signal: Signal, priority: Priority) -> Result<(), QueueError> {
        self.enqueue_signal(signal, priority)
            .await
            .map_err(QueueError::new)
    }

    async fn dequeue(&self, cancel: CancellationToken) -> Result<Option<Signal>, QueueError> {
        self.dequeue_signal(cancel).await.map_err(QueueError::new)
    }

    async fn close(&self) -> Result<(), QueueError> {
        self.inner.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn format_sdk_error<E, R>(err: &SdkError<E, R>) -> String
where
    E: std::fmt::Debug,
    R: std::fmt::Debug,
{
    format!("{err:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_round_trips_through_resolver() {
        let cfg = SqsQueueConfig {
            identity: SqsIdentity::Url(
                "https://sqs.us-east-1.amazonaws.com/123456789012/iter-signals".into(),
            ),
            region: None,
            endpoint_url: None,
            fifo: None,
            use_fips: None,
            use_dual_stack: None,
            sts_regional_endpoints: None,
            app_name: None,
            credentials: None,
            http_client: None,
            producer: None,
            consumer: None,
            retry: None,
            dlq: None,
        };
        let url = resolve_queue_url(&cfg).expect("resolve");
        assert_eq!(
            url,
            "https://sqs.us-east-1.amazonaws.com/123456789012/iter-signals"
        );
    }

    #[test]
    fn name_with_account_uses_region_for_default_host() {
        let cfg = SqsQueueConfig {
            identity: SqsIdentity::NameWithAccount {
                name: "iter".into(),
                account_id: "123456789012".into(),
            },
            region: Some("us-west-2".into()),
            endpoint_url: None,
            fifo: None,
            use_fips: None,
            use_dual_stack: None,
            sts_regional_endpoints: None,
            app_name: None,
            credentials: None,
            http_client: None,
            producer: None,
            consumer: None,
            retry: None,
            dlq: None,
        };
        let url = resolve_queue_url(&cfg).expect("resolve");
        assert_eq!(url, "https://sqs.us-west-2.amazonaws.com/123456789012/iter");
    }

    #[test]
    fn name_with_account_honours_endpoint_url() {
        let cfg = SqsQueueConfig {
            identity: SqsIdentity::NameWithAccount {
                name: "iter".into(),
                account_id: "000000000000".into(),
            },
            region: Some("us-east-1".into()),
            endpoint_url: Some("http://localhost:4566".into()),
            fifo: None,
            use_fips: None,
            use_dual_stack: None,
            sts_regional_endpoints: None,
            app_name: None,
            credentials: None,
            http_client: None,
            producer: None,
            consumer: None,
            retry: None,
            dlq: None,
        };
        let url = resolve_queue_url(&cfg).expect("resolve");
        assert_eq!(url, "http://localhost:4566/000000000000/iter");
    }

    #[test]
    fn name_with_account_requires_region() {
        let cfg = SqsQueueConfig {
            identity: SqsIdentity::NameWithAccount {
                name: "iter".into(),
                account_id: "123".into(),
            },
            region: None,
            endpoint_url: None,
            fifo: None,
            use_fips: None,
            use_dual_stack: None,
            sts_regional_endpoints: None,
            app_name: None,
            credentials: None,
            http_client: None,
            producer: None,
            consumer: None,
            retry: None,
            dlq: None,
        };
        let err = resolve_queue_url(&cfg).expect_err("requires region");
        assert!(matches!(err, SqsQueueError::Config(_)));
    }

    #[test]
    fn region_inferred_from_canonical_hostname() {
        assert_eq!(
            region_from_url("https://sqs.us-east-1.amazonaws.com/123/q").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(region_from_url("http://localhost:4566/000/q"), None);
    }

    #[test]
    fn fifo_template_missing_metadata_maps_to_sqs_error() {
        use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("workspace").expect("key"),
            MetadataValue::String("alpha".into()),
        );
        let signal = Signal::new(metadata);

        // The shared `MetadataSource::resolve` yields the neutral
        // `MissingMetadata`, which the SQS driver maps to its own
        // FIFO-template error via `From`.
        let missing = MetadataSource::FromMetadata("missing".into())
            .resolve(&signal)
            .expect_err("missing key");
        let mapped: SqsQueueError = missing.into();
        assert!(matches!(mapped, SqsQueueError::MissingTemplateMetadata { .. }));
    }
}
