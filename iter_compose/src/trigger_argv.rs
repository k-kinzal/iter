//! Translate a [`QueueDef`] into the `--queue-url` form consumed by service
//! runners (`iter run --service <name> --queue-url <url>`).
//!
//! Only queues with a stable, cross-process URL form are eligible:
//!
//! * `Memory` → not addressable across processes (returns `None`).
//! * `File { path }` → `file://<path>`.
//! * `Redis { url, key }` → `<url>?key=<key>` (key encoded as a query parameter).
//!
//! Cloud and shell-driven queues (`Sqs`, `PubSub`, `Kafka`, `Kinesis`,
//! `ServiceBus`, `Shell`) require the full Iterfile to reconstruct, so they
//! return `None`; the caller is expected to fall back to the in-process path.

use iter_language::QueueDef;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

/// Translate a `QueueDef` into a `--queue-url` value.
///
/// Returns `None` for queues that have no cross-process URL form (memory,
/// cloud backends, shell).
#[must_use]
pub fn queue_to_url(decl: &QueueDef) -> Option<String> {
    match decl {
        QueueDef::File { path } => Some(format!("file://{path}")),
        QueueDef::Redis { url, key } => {
            if key.is_empty() {
                Some(url.clone())
            } else {
                let encoded_key = utf8_percent_encode(key, NON_ALPHANUMERIC);
                let separator = if url.contains('?') { '&' } else { '?' };
                Some(format!("{url}{separator}key={encoded_key}"))
            }
        }
        // Memory is in-process only; cloud and shell backends require the
        // full Iterfile to reconstruct (no single-URL form).
        QueueDef::Memory
        | QueueDef::Shell { .. }
        | QueueDef::Sqs(_)
        | QueueDef::PubSub(_)
        | QueueDef::Kafka(_)
        | QueueDef::Kinesis(_)
        | QueueDef::ServiceBus(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_to_url_memory_returns_none() {
        assert!(queue_to_url(&QueueDef::Memory).is_none());
    }

    #[test]
    fn queue_to_url_file_emits_file_scheme() {
        let url = queue_to_url(&QueueDef::File {
            path: "/tmp/q".into(),
        })
        .expect("file");
        assert_eq!(url, "file:///tmp/q");
    }

    #[test]
    fn queue_to_url_redis_appends_key_query_param() {
        let url = queue_to_url(&QueueDef::Redis {
            url: "redis://h:6379".into(),
            key: "main".into(),
        })
        .expect("redis");
        assert_eq!(url, "redis://h:6379?key=main");
    }

    #[test]
    fn queue_to_url_redis_merges_existing_query() {
        let url = queue_to_url(&QueueDef::Redis {
            url: "redis://h:6379?db=0".into(),
            key: "main".into(),
        })
        .expect("redis");
        assert_eq!(url, "redis://h:6379?db=0&key=main");
    }

    #[test]
    fn queue_to_url_redis_encodes_special_chars_in_key() {
        let url = queue_to_url(&QueueDef::Redis {
            url: "redis://h:6379".into(),
            key: "main&aux".into(),
        })
        .expect("redis");
        assert_eq!(url, "redis://h:6379?key=main%26aux");
    }

    #[test]
    fn queue_to_url_redis_empty_key_omitted() {
        let url = queue_to_url(&QueueDef::Redis {
            url: "redis://h:6379".into(),
            key: String::new(),
        })
        .expect("redis");
        assert_eq!(url, "redis://h:6379");
    }
}
