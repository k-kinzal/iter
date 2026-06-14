//! `iter enqueue` — push a single [`Signal`] onto a queue.
//!
//! Resolves a queue from one of three sources, in this priority:
//!
//! 1. `--queue-url URL` — parsed into a [`QueueDescriptor`] and connected
//!    through the core [`connect`] boundary (the one place queue URLs are
//!    resolved).
//! 2. `-f PATH` (or auto-detected `./compose.iter`/`./Iterfile`) — built
//!    from the file's queue declaration. Compose files with multiple
//!    queues require `--queue NAME` to disambiguate.
//!
//! Metadata pairs are parsed as `KEY=VALUE` and stored as
//! [`MetadataValue::String`]. Integer / Bool / JSON values are not accepted;
//! the runner template renderer interpolates everything as strings anyway.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{ComposeError, is_compose_filename, load_compose, queue_from_def};
use iter_core::Queue;
use iter_core::queue::{Priority, QueueAddressError, QueueDescriptor, connect};
use iter_core::signal::{Metadata, MetadataError, MetadataKey, MetadataValue, Signal};
use iter_language::{Compose, NamedQueue, QueueDef};
use thiserror::Error;

use crate::cli::{EnqueueArgs, EnqueuePriority};
use crate::dispatch::compose::compose_error_exit_code;
use crate::dispatch::load::DEFAULT_ITERFILE;
use crate::output::{IntoExitCode, cli_println, exit_codes};
use crate::telemetry;
use crate::tracing_preferences::TracingPreferences;

/// Errors produced by `iter enqueue`.
#[derive(Debug, Error)]
pub(crate) enum EnqueueCmdError {
    /// Resolving / building the queue failed.
    #[error(transparent)]
    Compose(#[from] ComposeError),
    /// A `-m KEY=VALUE` argument did not contain a `=` separator.
    #[error("metadata `{0}` is missing `=`; expected KEY=VALUE")]
    MetadataMissingSeparator(String),
    /// A metadata key was rejected by [`MetadataKey::new`].
    #[error("invalid metadata key: {0}")]
    MetadataKey(#[from] MetadataError),
    /// Neither `--queue-url`, `-f`, nor an auto-detected file is available.
    #[error(
        "no queue source: pass --queue-url URL, -f PATH, or run from a directory \
         containing compose.iter or Iterfile"
    )]
    NoQueueSource,
    /// The compose file declares multiple queues but `--queue NAME` was not
    /// supplied.
    #[error("compose file declares multiple queues ({names}); pass --queue NAME to choose one")]
    AmbiguousQueue {
        /// Comma-separated declared queue names, for the diagnostic.
        names: String,
    },
    /// The compose file does not contain a queue with the requested name.
    #[error("queue `{name}` is not declared in {}", path.display())]
    UnknownQueue {
        /// Resolved compose path.
        path: PathBuf,
        /// Requested queue name.
        name: String,
    },
    /// The compose file declared no queues at all.
    #[error("compose file {} declares no queues", path.display())]
    NoQueuesInCompose {
        /// Resolved compose path.
        path: PathBuf,
    },
    /// The provided `--queue` flag was used with an Iterfile (single-queue file).
    #[error("--queue is only meaningful when -f points at a compose.iter file")]
    QueueFlagNotApplicable,
    /// Pushing the signal onto the queue failed.
    #[error("queueing signal: {0}")]
    Queue(String),
    /// The `--queue-url` could not be parsed as a queue address (unknown
    /// scheme, missing `file://` path, or empty redis `?key=`).
    #[error(transparent)]
    QueueUrl(#[from] QueueAddressError),
    /// Iterfile loaded via `-f` had no `queue` section.
    #[error("iterfile {} has no `queue` section", path.display())]
    IterfileMissingQueue {
        /// Iterfile path.
        path: PathBuf,
    },
    /// Loading or parsing an Iterfile failed.
    #[error(transparent)]
    Load(#[from] super::load::LoadError),
}

impl IntoExitCode for EnqueueCmdError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::MetadataMissingSeparator(_)
            | Self::MetadataKey(_)
            | Self::NoQueueSource
            | Self::AmbiguousQueue { .. }
            | Self::UnknownQueue { .. }
            | Self::NoQueuesInCompose { .. }
            | Self::QueueFlagNotApplicable
            | Self::QueueUrl(_)
            | Self::IterfileMissingQueue { .. } => exit_codes::USER_INPUT,
            Self::Load(e) => e.exit_code(),
            Self::Compose(e) => compose_error_exit_code(e),
            Self::Queue(_) => exit_codes::RUNTIME,
        }
    }
}

/// Handle `iter enqueue`.
///
/// Returns `Ok(())` once the signal has been accepted by the queue. The
/// signal id is printed to stdout on success.
///
/// # Errors
///
/// See [`EnqueueCmdError`].
pub(crate) async fn run_enqueue(
    args: EnqueueArgs,
    prefs: TracingPreferences,
) -> Result<(), EnqueueCmdError> {
    let _telemetry_guard = telemetry::init(false, &prefs);

    let queue = resolve_queue(&args).await?;
    let metadata = parse_metadata(&args.metadata)?;
    let priority = map_priority(args.priority);
    let signal = Signal::new(metadata);
    let id = signal.id();

    queue
        .enqueue(signal, priority)
        .await
        .map_err(|e| EnqueueCmdError::Queue(e.to_string()))?;

    cli_println!("{id}");
    Ok(())
}

fn map_priority(p: EnqueuePriority) -> Priority {
    match p {
        EnqueuePriority::Low => Priority::LOW,
        EnqueuePriority::Normal => Priority::NORMAL,
        EnqueuePriority::High => Priority::HIGH,
        EnqueuePriority::Critical => Priority::CRITICAL,
    }
}

fn parse_metadata(items: &[String]) -> Result<Metadata, EnqueueCmdError> {
    let mut metadata = Metadata::new();
    for item in items {
        let (k, v) = item
            .split_once('=')
            .ok_or_else(|| EnqueueCmdError::MetadataMissingSeparator(item.clone()))?;
        let key = MetadataKey::new(k)?;
        metadata.insert(key, MetadataValue::String(v.to_owned()));
    }
    Ok(metadata)
}

async fn resolve_queue(args: &EnqueueArgs) -> Result<Arc<dyn Queue>, EnqueueCmdError> {
    if let Some(url) = args.queue_url.as_deref() {
        if args.queue.is_some() {
            return Err(EnqueueCmdError::QueueFlagNotApplicable);
        }
        // `memory://` builds a fresh in-process queue for this single
        // enqueue; `file://` / `redis://` dial the addressable backend.
        let descriptor = QueueDescriptor::from_url(url)?;
        return connect(&descriptor)
            .await
            .map_err(|e| EnqueueCmdError::Queue(e.to_string()));
    }

    let path = match args.file.as_deref() {
        Some(p) => p.to_path_buf(),
        None => match autodetect_file() {
            Some(p) => p,
            None => return Err(EnqueueCmdError::NoQueueSource),
        },
    };

    if is_compose_filename(&path) {
        resolve_from_compose(&path, args.queue.as_deref())
    } else {
        if args.queue.is_some() {
            return Err(EnqueueCmdError::QueueFlagNotApplicable);
        }
        let decl = load_iterfile_queue(&path)?;
        Ok(queue_from_def(&decl).map_err(ComposeError::from)?)
    }
}

fn load_iterfile_queue(path: &Path) -> Result<QueueDef, EnqueueCmdError> {
    let loaded = super::load::load_iterfile(Some(path))?;
    loaded
        .iterfile
        .queues
        .into_iter()
        .next()
        .map(|q| q.node.decl)
        .ok_or_else(|| EnqueueCmdError::IterfileMissingQueue {
            path: path.to_path_buf(),
        })
}

fn autodetect_file() -> Option<PathBuf> {
    let compose = PathBuf::from(crate::DEFAULT_COMPOSE_FILE);
    if compose.exists() {
        return Some(compose);
    }
    let iterfile = PathBuf::from(DEFAULT_ITERFILE);
    if iterfile.exists() {
        return Some(iterfile);
    }
    None
}

fn resolve_from_compose(
    path: &Path,
    queue_name: Option<&str>,
) -> Result<Arc<dyn Queue>, EnqueueCmdError> {
    let root: Compose = load_compose(path)?;
    if root.queues.is_empty() {
        return Err(EnqueueCmdError::NoQueuesInCompose {
            path: path.to_path_buf(),
        });
    }

    let decl = match queue_name {
        Some(name) => {
            find_named_queue(&root, name).ok_or_else(|| EnqueueCmdError::UnknownQueue {
                path: path.to_path_buf(),
                name: name.to_owned(),
            })?
        }
        None => {
            if root.queues.len() == 1 {
                &root.queues[0].node
            } else {
                let names = root
                    .queues
                    .iter()
                    .map(|q| q.node.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(EnqueueCmdError::AmbiguousQueue { names });
            }
        }
    };

    Ok(queue_from_def(&decl.decl).map_err(ComposeError::from)?)
}

fn find_named_queue<'a>(root: &'a Compose, name: &str) -> Option<&'a NamedQueue> {
    root.queues.iter().map(|q| &q.node).find(|q| q.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_metadata_accepts_kv_pairs() {
        let m = parse_metadata(&["a=1".into(), "b=hello".into()]).expect("ok");
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.get_str("a"),
            Some(&MetadataValue::String("1".to_string()))
        );
        assert_eq!(
            m.get_str("b"),
            Some(&MetadataValue::String("hello".to_string()))
        );
    }

    #[test]
    fn parse_metadata_value_may_contain_equals() {
        let m = parse_metadata(&["expr=a=b=c".into()]).expect("ok");
        assert_eq!(
            m.get_str("expr"),
            Some(&MetadataValue::String("a=b=c".to_string()))
        );
    }

    #[test]
    fn parse_metadata_rejects_missing_separator() {
        let err = parse_metadata(&["bare".into()]).expect_err("must fail");
        assert!(matches!(err, EnqueueCmdError::MetadataMissingSeparator(_)));
    }

    #[test]
    fn parse_metadata_rejects_invalid_key() {
        let err = parse_metadata(&["not a key=v".into()]).expect_err("must fail");
        assert!(matches!(err, EnqueueCmdError::MetadataKey(_)));
    }

    #[test]
    fn map_priority_translates_all_variants() {
        assert_eq!(map_priority(EnqueuePriority::Low), Priority::LOW);
        assert_eq!(map_priority(EnqueuePriority::Normal), Priority::NORMAL);
        assert_eq!(map_priority(EnqueuePriority::High), Priority::HIGH);
        assert_eq!(map_priority(EnqueuePriority::Critical), Priority::CRITICAL);
    }

    /// Pin keyword-set parity between core's canonical
    /// [`Priority::from_keyword`]/[`Priority::keyword`] mapping and the
    /// language's [`iter_language::PriorityKeyword`]. Lives cli-side because it
    /// is the one place that sees both crates. The exhaustive `match` forces a
    /// compile error if a `PriorityKeyword` variant is added without updating
    /// core's keyword mapping here.
    #[test]
    fn priority_keyword_parity_with_language() {
        use iter_language::PriorityKeyword;

        fn keyword_of(kw: PriorityKeyword) -> &'static str {
            match kw {
                PriorityKeyword::Low => "low",
                PriorityKeyword::Normal => "normal",
                PriorityKeyword::High => "high",
                PriorityKeyword::Critical => "critical",
            }
        }

        let pairs = [
            (PriorityKeyword::Low, Priority::LOW),
            (PriorityKeyword::Normal, Priority::NORMAL),
            (PriorityKeyword::High, Priority::HIGH),
            (PriorityKeyword::Critical, Priority::CRITICAL),
        ];
        for (kw, expected) in pairs {
            let s = keyword_of(kw);
            // Core accepts every language keyword, mapping to the same level…
            assert_eq!(Priority::from_keyword(s), Some(expected));
            // …and rounds back to the same keyword string.
            assert_eq!(expected.keyword(), s);
        }
        // Core accepts no keyword the language does not, and rejects unknowns.
        assert_eq!(Priority::from_keyword("bogus"), None);
    }

    fn url_args(url: &str) -> EnqueueArgs {
        EnqueueArgs {
            queue_url: Some(url.to_owned()),
            file: None,
            queue: None,
            metadata: vec![],
            priority: EnqueuePriority::Normal,
        }
    }

    #[tokio::test]
    async fn queue_url_memory_colon_alias() {
        let q = resolve_queue(&url_args("memory:"))
            .await
            .expect("memory: alias");
        drop(q);
    }

    #[tokio::test]
    async fn queue_url_memory_scheme() {
        let q = resolve_queue(&url_args("memory://"))
            .await
            .expect("memory://");
        drop(q);
    }

    #[tokio::test]
    async fn queue_url_file_requires_path() {
        let err = resolve_queue(&url_args("file://"))
            .await
            .err()
            .expect("must fail");
        assert!(matches!(
            err,
            EnqueueCmdError::QueueUrl(QueueAddressError::FileUrlMissingPath)
        ));
    }

    #[tokio::test]
    async fn queue_url_file_absolute() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("queue");
        let url = format!("file://{}", path.display());
        let q = resolve_queue(&url_args(&url)).await.expect("file queue");
        drop(q);
    }

    #[tokio::test]
    async fn queue_url_unknown_scheme_errors() {
        let err = resolve_queue(&url_args("amqp://x"))
            .await
            .err()
            .expect("must fail");
        assert!(matches!(
            err,
            EnqueueCmdError::QueueUrl(QueueAddressError::UnsupportedScheme(_))
        ));
    }

    // The redis `?key=` parsing / percent-decoding lives in
    // `iter_core::queue::QueueAddress` and is tested there — `--queue-url`
    // resolution is the thin connector over it.
}
