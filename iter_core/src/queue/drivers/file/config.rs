//! On-disk layout constants for [`FileQueue`](super::FileQueue).

use std::time::Duration;

/// Subdirectory holding signals that have been enqueued and are waiting to
/// be dequeued. Files here are committed; their lex-smallest name is
/// always the next signal to hand out.
pub(super) const PENDING_DIR: &str = "pending";

/// Subdirectory holding two kinds of transient files: `.partial` files
/// being written by an in-flight enqueue, and `.claim-*` files being read
/// by an in-flight dequeue. Anything that survives a process exit here is
/// orphan state and is swept on the next open.
pub(super) const TMP_DIR: &str = "tmp";

/// Suffix appended to a tmp file while its contents are being written and
/// fsynced. Removed on successful rename into `pending/`.
pub(super) const PARTIAL_SUFFIX: &str = ".partial";

/// Substring marking a tmp file as a claim. The full suffix is
/// `.claim-<pid>-<seq>` so that two consumers in different processes
/// cannot collide on the same scratch name.
pub(super) const CLAIM_INFIX: &str = ".claim-";

/// Backstop poll interval for `dequeue`. The in-process [`Notify`](tokio::sync::Notify)
/// handles same-process producer wakes and the `notify` watcher handles
/// cross-process wakes; this poll only matters when both miss — for
/// example on filesystems where `notify`'s native backend cannot observe
/// the directory (NFS, some FUSE mounts).
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(50);
