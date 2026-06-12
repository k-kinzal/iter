//! Cross-backend dead-letter-queue policy.
//!
//! Backends fall into two groups:
//!
//! * **Native DLQ** (SQS): the broker handles dead-lettering; iter only
//!   observes via [`DlqPolicy::Native`].
//! * **No native DLQ**: iter implements republishing via
//!   [`DlqPolicy::IterRepublish`] with explicit receive-count tracking, to one
//!   of the [`DlqTarget`]s below.

use serde::{Deserialize, Serialize};

/// Where to land poison records when iter republishes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlqTarget {
    /// AWS SQS queue.
    Sqs {
        /// Fully-qualified SQS queue URL.
        queue_url: String,
        /// AWS region.
        region: String,
    },
    /// AWS S3 bucket. Poison records land as one object per signal under
    /// `prefix/<signal-id>.json`.
    S3 {
        /// Bucket name.
        bucket: String,
        /// Key prefix (must end with `/` if you want a directory).
        prefix: String,
        /// AWS region.
        region: String,
    },
    /// Local file. Each poison record is appended as one line of JSON.
    File {
        /// File path. Created with `0640` permissions if it doesn't exist.
        path: String,
    },
}

/// Cross-backend DLQ policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DlqPolicy {
    /// No DLQ behaviour. Poison records are dropped on the floor after the
    /// configured retry policy gives up.
    #[default]
    None,
    /// The backend has a native DLQ already configured on the entity (SQS
    /// redrive policy, Service Bus dead-letter sub-queue, Pub/Sub
    /// subscription DLQ). iter does not republish; it just observes.
    Native,
    /// iter writes poison records to a [`DlqTarget`] after `max_receive_count`
    /// failed attempts. Used by backends with no native DLQ.
    IterRepublish {
        /// How many times a record may be delivered before iter routes it to
        /// `target`.
        max_receive_count: u32,
        /// Where to send the poison record.
        target: DlqTarget,
        /// Whether to copy the original headers/attributes alongside the
        /// payload.
        include_headers: bool,
        /// Optional template (Tera) used for the failure-reason field on the
        /// republished record. Examples: `{{error.kind}} - {{error.message}}`.
        reason_template: Option<String>,
    },
}

/// Top-level discriminator for `dlq.kind = "..."` declaration fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DlqKind {
    /// [`DlqPolicy::None`].
    None,
    /// [`DlqPolicy::Native`].
    Native,
    /// [`DlqPolicy::IterRepublish`].
    IterRepublish,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(DlqPolicy::default(), DlqPolicy::None);
    }

    #[test]
    fn iter_republish_carries_full_config() {
        let p = DlqPolicy::IterRepublish {
            max_receive_count: 5,
            target: DlqTarget::File {
                path: "/tmp/iter-dlq.jsonl".into(),
            },
            include_headers: true,
            reason_template: Some("{{error.kind}}".into()),
        };
        match p {
            DlqPolicy::IterRepublish {
                max_receive_count, ..
            } => assert_eq!(max_receive_count, 5),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
