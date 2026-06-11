//! [`QueueDescriptor`] — everything another process needs to *connect* to a
//! Queue.
//!
//! A descriptor is a backend tag plus the resolved parameters required to dial
//! the queue, including a **resolved-credential slot** that is redacted in
//! `Debug` and never serialized into a human-facing form. It is the carrier
//! the addressable backends (`memory`/`file`/`redis`) and SQS connect from via
//! [`connect`](crate::queue::connect); `ShellQueue` has no descriptor form (it
//! is built only from the full definition).
//!
//! Unlike a bare URL, the descriptor can carry SQS, which has no single-URL
//! spelling (region / endpoint / `fifo` / `message_group_id` / credentials).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::queue::address::{QueueAddress, QueueAddressError};
use crate::queue::metadata_source::MetadataSource;

/// Everything needed to connect to a queue from another process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum QueueDescriptor {
    /// In-process queue (`memory://`). Only connectable in-process.
    Memory,
    /// Directory-based queue (`file://`).
    File {
        /// Filesystem path of the queue directory.
        path: String,
    },
    /// Redis sorted-set queue (`redis://`).
    Redis {
        /// Connection URL with the `?key=` query stripped.
        url: String,
        /// Redis key namespace.
        key: String,
    },
    /// Amazon SQS — no single-URL spelling, so carried structurally.
    Sqs(SqsDescriptor),
}

/// The connect-relevant subset of an SQS queue declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqsDescriptor {
    /// Fully-qualified queue URL, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_url: Option<String>,
    /// Queue name (paired with `account_id` when the URL is not known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_name: Option<String>,
    /// 12-digit AWS account id owning the queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Service region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Custom endpoint URL (`LocalStack` / VPC endpoints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    /// Override FIFO-mode detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fifo: Option<bool>,
    /// FIFO `MessageGroupId` source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_group_id: Option<MetadataSource>,
    /// Resolved credentials. Redacted in `Debug`; the operator resolves any
    /// `SecretExpr` before building the descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<ResolvedQueueCredentials>,
}

/// Resolved (literal) AWS-style credentials carried in a descriptor.
///
/// `Debug` is hand-written to redact the secret material so a descriptor can
/// be logged without leaking credentials (the secret value never appears in
/// `format!("{:?}", …)`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedQueueCredentials {
    /// `AWS_ACCESS_KEY_ID` equivalent.
    pub access_key_id: String,
    /// `AWS_SECRET_ACCESS_KEY` equivalent.
    pub secret_access_key: String,
    /// Optional STS session token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl fmt::Debug for ResolvedQueueCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redact every field: the access key id is identifying and the
        // secret/token are sensitive. The struct's presence is the only
        // thing observable.
        f.debug_struct("ResolvedQueueCredentials")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &self.session_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl QueueDescriptor {
    /// Build a descriptor from a queue URL (the addressable subset).
    ///
    /// SQS has no URL form; use the [`SqsDescriptor`] constructor for it.
    ///
    /// # Errors
    ///
    /// Returns [`QueueAddressError`] for the same reasons as
    /// [`QueueAddress::parse`].
    pub fn from_url(url: &str) -> Result<Self, QueueAddressError> {
        Ok(match QueueAddress::parse(url)? {
            QueueAddress::Memory => Self::Memory,
            QueueAddress::File { path } => Self::File { path },
            QueueAddress::Redis { url, key } => Self::Redis { url, key },
        })
    }

    /// Whether a *separate* process can dial this queue (everything but
    /// `memory`).
    #[must_use]
    pub fn is_addressable(&self) -> bool {
        !matches!(self, Self::Memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_url_maps_addressable_backends() {
        assert_eq!(QueueDescriptor::from_url("memory://").unwrap(), QueueDescriptor::Memory);
        assert_eq!(
            QueueDescriptor::from_url("file:///tmp/q").unwrap(),
            QueueDescriptor::File { path: "/tmp/q".into() }
        );
        assert_eq!(
            QueueDescriptor::from_url("redis://h?key=k").unwrap(),
            QueueDescriptor::Redis { url: "redis://h".into(), key: "k".into() }
        );
    }

    #[test]
    fn sqs_descriptor_round_trips_through_serde() {
        let descriptor = QueueDescriptor::Sqs(SqsDescriptor {
            queue_url: Some("https://sqs.us-east-1.amazonaws.com/123456789012/iter.fifo".into()),
            queue_name: None,
            account_id: None,
            region: Some("us-east-1".into()),
            endpoint_url: None,
            fifo: Some(true),
            message_group_id: Some(MetadataSource::FromMetadata("group".into())),
            credentials: Some(ResolvedQueueCredentials {
                access_key_id: "AKIA".into(),
                secret_access_key: "SECRETVALUE".into(),
                session_token: None,
            }),
        });
        let json = serde_json::to_string(&descriptor).expect("serialize");
        let back: QueueDescriptor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(descriptor, back);
        // message_group_id survives the round trip.
        match &back {
            QueueDescriptor::Sqs(d) => assert_eq!(
                d.message_group_id,
                Some(MetadataSource::FromMetadata("group".into()))
            ),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn debug_redacts_the_credential_slot() {
        let descriptor = QueueDescriptor::Sqs(SqsDescriptor {
            queue_url: Some("https://sqs.local/q".into()),
            queue_name: None,
            account_id: None,
            region: None,
            endpoint_url: None,
            fifo: None,
            message_group_id: None,
            credentials: Some(ResolvedQueueCredentials {
                access_key_id: "AKIAEXAMPLE".into(),
                secret_access_key: "SUPERSECRETVALUE".into(),
                session_token: Some("TOKENVALUE".into()),
            }),
        });
        let debug = format!("{descriptor:?}");
        assert!(!debug.contains("SUPERSECRETVALUE"), "secret leaked: {debug}");
        assert!(!debug.contains("AKIAEXAMPLE"), "access key leaked: {debug}");
        assert!(!debug.contains("TOKENVALUE"), "session token leaked: {debug}");
        assert!(debug.contains("<redacted>"));
    }
}
