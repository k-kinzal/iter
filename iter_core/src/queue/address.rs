//! [`QueueAddress`] — the small URL grammar naming which Queue a process
//! talks to.
//!
//! One concept, one home: parse (`memory://`, `file://<path>`,
//! `redis://…?key=…`), print, and the **addressability** predicate that says
//! whether a *separate* process can dial the queue. The redis `?key=` query is
//! parsed exactly once here; an empty `?key=` is a diagnostic error, not a
//! silent default.

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use thiserror::Error;

/// Default Redis key namespace when `?key=` is omitted entirely.
const DEFAULT_REDIS_KEY: &str = "iter";

/// Characters percent-encoded when emitting a Redis `?key=` value.
const KEY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'&')
    .add(b'?')
    .add(b'#')
    .add(b'=')
    .add(b'%');

/// The addressable subset of queue backends, named by URL.
///
/// `Shell` and `Sqs` are deliberately absent: they are not URL-addressable
/// (shell needs scripts; SQS needs a structured
/// [`QueueDescriptor`](crate::queue::QueueDescriptor)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAddress {
    /// `memory://` — in-process queue. **Not** dialable from another process.
    Memory,
    /// `file://<path>` — directory-based queue.
    File {
        /// Filesystem path of the queue directory.
        path: String,
    },
    /// `redis://…` / `rediss://…` — Redis sorted-set queue.
    Redis {
        /// Connection URL with the `?key=` query stripped.
        url: String,
        /// Redis key namespace (`?key=`; defaults to `iter`).
        key: String,
    },
}

/// Errors parsing a [`QueueAddress`] from a URL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum QueueAddressError {
    /// The URL scheme is not one of `memory`/`file`/`redis`/`rediss`.
    #[error("unsupported queue url `{0}`; expected one of memory://, file:///path, redis://..., rediss://...")]
    UnsupportedScheme(String),
    /// `file://` URL with an empty path component.
    #[error("file:// queue url is missing a path")]
    FileUrlMissingPath,
    /// `redis://…?key=` with an empty value — the namespace would be empty.
    /// Omit `?key=` to take the default (`iter`), or supply a non-empty key.
    #[error("redis queue url has an empty `?key=`; omit it for the default `iter`, or supply a key")]
    EmptyRedisKey,
}

impl QueueAddress {
    /// Parse a queue URL into a [`QueueAddress`].
    ///
    /// # Errors
    ///
    /// Returns [`QueueAddressError`] when the scheme is unknown, a `file://`
    /// path is empty, or a redis `?key=` is present but empty.
    pub fn parse(url: &str) -> Result<Self, QueueAddressError> {
        if url == "memory://" || url == "memory:" {
            return Ok(Self::Memory);
        }
        if let Some(rest) = url.strip_prefix("file://") {
            if rest.is_empty() {
                return Err(QueueAddressError::FileUrlMissingPath);
            }
            return Ok(Self::File {
                path: rest.to_string(),
            });
        }
        if url.starts_with("redis://") || url.starts_with("rediss://") {
            let (base, key) = match url.split_once('?') {
                Some((base, query)) => (base.to_string(), parse_redis_key(query)?),
                None => (url.to_string(), DEFAULT_REDIS_KEY.to_string()),
            };
            return Ok(Self::Redis { url: base, key });
        }
        Err(QueueAddressError::UnsupportedScheme(url.to_owned()))
    }

    /// Render this address back to its URL form.
    ///
    /// Round-trips with [`QueueAddress::parse`]: a redis address whose key is
    /// the default emits no `?key=`; a custom key is percent-encoded.
    #[must_use]
    pub fn to_url(&self) -> String {
        match self {
            Self::Memory => "memory://".to_string(),
            Self::File { path } => format!("file://{path}"),
            Self::Redis { url, key } if key == DEFAULT_REDIS_KEY => url.clone(),
            Self::Redis { url, key } => {
                let encoded = utf8_percent_encode(key, KEY_ENCODE_SET);
                format!("{url}?key={encoded}")
            }
        }
    }

    /// Whether a *separate* process can dial this queue.
    ///
    /// `memory://` is in-process only (R3): a trigger subprocess cannot reach
    /// an in-process queue, so plan-time rejection of a non-addressable queue
    /// for spawning uses this predicate.
    #[must_use]
    pub fn is_addressable(&self) -> bool {
        !matches!(self, Self::Memory)
    }
}

/// Extract the `key` parameter out of a redis query string, rejecting an empty
/// value. Other parameters are ignored.
fn parse_redis_key(query: &str) -> Result<String, QueueAddressError> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == "key"
        {
            if v.is_empty() {
                return Err(QueueAddressError::EmptyRedisKey);
            }
            return Ok(percent_decode_str(v).decode_utf8_lossy().into_owned());
        }
    }
    Ok(DEFAULT_REDIS_KEY.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory() {
        assert_eq!(QueueAddress::parse("memory://").unwrap(), QueueAddress::Memory);
        assert_eq!(QueueAddress::parse("memory:").unwrap(), QueueAddress::Memory);
    }

    #[test]
    fn parse_file_requires_path() {
        assert_eq!(
            QueueAddress::parse("file:///tmp/q").unwrap(),
            QueueAddress::File {
                path: "/tmp/q".into()
            }
        );
        assert_eq!(
            QueueAddress::parse("file://").unwrap_err(),
            QueueAddressError::FileUrlMissingPath
        );
    }

    #[test]
    fn parse_redis_default_and_custom_key() {
        assert_eq!(
            QueueAddress::parse("redis://host:6379/0").unwrap(),
            QueueAddress::Redis {
                url: "redis://host:6379/0".into(),
                key: "iter".into()
            }
        );
        assert_eq!(
            QueueAddress::parse("redis://host:6379/0?key=my%20queue").unwrap(),
            QueueAddress::Redis {
                url: "redis://host:6379/0".into(),
                key: "my queue".into()
            }
        );
    }

    #[test]
    fn parse_empty_redis_key_is_an_error() {
        assert_eq!(
            QueueAddress::parse("redis://host?key=").unwrap_err(),
            QueueAddressError::EmptyRedisKey
        );
    }

    #[test]
    fn parse_unknown_scheme_errors() {
        assert!(matches!(
            QueueAddress::parse("ftp://host").unwrap_err(),
            QueueAddressError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn to_url_round_trips() {
        for url in [
            "memory://",
            "file:///tmp/q",
            "redis://host:6379/0",
        ] {
            let addr = QueueAddress::parse(url).unwrap();
            assert_eq!(addr.to_url(), url, "round-trip for {url}");
        }
    }

    #[test]
    fn to_url_emits_custom_key_percent_encoded() {
        let addr = QueueAddress::Redis {
            url: "redis://h".into(),
            key: "main aux".into(),
        };
        assert_eq!(addr.to_url(), "redis://h?key=main%20aux");
        // and it parses back to the same key
        assert_eq!(QueueAddress::parse(&addr.to_url()).unwrap(), addr);
    }

    #[test]
    fn memory_is_not_addressable() {
        assert!(!QueueAddress::Memory.is_addressable());
        assert!(QueueAddress::File { path: "/q".into() }.is_addressable());
        assert!(
            QueueAddress::Redis {
                url: "redis://h".into(),
                key: "iter".into()
            }
            .is_addressable()
        );
    }
}
