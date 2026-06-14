//! `~/.iter/proc/.locks/<name>` — name-uniqueness for `Process` records.
//!
//! Each live registration is a regular file under `.locks/` whose body is
//!
//! ```text
//! <ulid>\n<creation_at_rfc3339>\n
//! ```
//!
//! and whose holder keeps a `flock(LOCK_EX)` on its file descriptor for the
//! lifetime of the process. The file *and* the kernel-level lock together
//! constitute "this name is in use right now".
//!
//! # Lock-before-publish, hold-fd-after-link (rev17 §I)
//!
//! The publication sequence is:
//!
//! 1. `openat(dirfd, ".<name>.<32hex>.tmp", O_CREAT|O_EXCL|O_RDWR|O_CLOEXEC|O_NOFOLLOW, 0600)`
//! 2. `write_all(body)` + `fsync(fd)`
//! 3. `flock(fd, LOCK_EX)` — uncontended on a freshly-created tmp.
//! 4. `linkat(dirfd, tmp, dirfd, name, 0)` — create-fail-if-exists.
//! 5. On success: `unlinkat(dirfd, tmp)` + `fsync(dirfd)` + return [`LockGuard`].
//! 6. On `EEXIST`: enter [`stale::stale_check`], which may unlink-and-retry.
//! 7. On `ENOENT` (tmp source vanished — janitor race): retry with a fresh
//!    suffix, up to 3 times, then [`crate::process::error::RegistryError::TmpRetryExhausted`].
//! 8. On `EPERM`/`EOPNOTSUPP`/`EXDEV`:
//!    [`crate::process::error::RegistryError::UnsupportedFilesystem`].
//!
//! The held fd's `flock` blocks any rival acquirer's `stale_check` flock
//! until the holder dies (kernel auto-release on close) or explicitly calls
//! [`LockGuard::release`].
//!
//! # Layering
//!
//! ```text
//!   syscall              <- libc wrappers (fstat, flock, dup, write_then_sync …)
//!   name                 <- validate_name + .tmp filename round-trip
//!   guard                <- LockGuard owned handle + release
//!   stale                <- stale_check (used by acquire on EEXIST)
//!   janitor              <- background sweep (used by acquire on entry)
//!   acquire              <- the publish loop, ties everything together
//! ```
//!
//! # Sync API
//!
//! Every helper here is synchronous. The async registry layer
//! ([`crate::process::registry`]) wraps `acquire` in `tokio::task::spawn_blocking`
//! so the flock hold never crosses an `.await`.

mod acquire;
mod guard;
mod janitor;
mod name;
mod release;
mod stale;
mod syscall;

pub(crate) use acquire::acquire;
pub(crate) use guard::LockGuard;
pub(crate) use release::release_by_id;

use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use crate::process::error::RegistryError;

/// Construct the locks directory at `<root>/.locks/` with mode 0700 and
/// return an owned `dirfd` opened with `O_DIRECTORY|O_CLOEXEC`.
#[cfg(unix)]
pub(crate) fn open_locks_dir(root: &Path) -> Result<(PathBuf, OwnedFd), RegistryError> {
    use crate::process::paths::{DIR_MODE, LOCKS_SUBDIR, ensure_dir_with_mode};
    use std::os::unix::fs::OpenOptionsExt;
    let dir = root.join(LOCKS_SUBDIR);
    ensure_dir_with_mode(&dir, DIR_MODE).map_err(|e| match e {
        crate::process::error::ProcessError::Io(io_err) => RegistryError::Io(io_err),
        other => RegistryError::Io(io::Error::other(other.to_string())),
    })?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(&dir)
        .map_err(RegistryError::Io)?;
    Ok((dir, file.into()))
}

#[cfg(not(unix))]
pub(crate) fn open_locks_dir(_root: &Path) -> Result<(PathBuf, OwnedFd), RegistryError> {
    Err(RegistryError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "open_locks_dir is unix-only",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::id::ProcessId;
    use crate::process::paths::{DIR_MODE, ensure_dir_with_mode};
    use chrono::Utc;
    use std::os::fd::AsFd;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn setup_root() -> (TempDir, PathBuf, PathBuf, OwnedFd) {
        let tmp = TempDir::new().unwrap();
        let proc_root = tmp.path().to_path_buf();
        ensure_dir_with_mode(&proc_root, DIR_MODE).unwrap();
        let (locks_path, locks_fd) = open_locks_dir(&proc_root).unwrap();
        (tmp, proc_root, locks_path, locks_fd)
    }

    #[cfg(unix)]
    #[test]
    fn acquire_then_release_round_trip() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let ulid = ProcessId::generate();
        let guard =
            acquire(locks_fd.as_fd(), &locks_path, &proc_root, "alpha", ulid).expect("acquire");
        let entry = locks_path.join("alpha");
        assert!(entry.exists(), "lock file should exist");
        let body = std::fs::read_to_string(&entry).unwrap();
        assert!(body.starts_with(&ulid.to_string()), "body starts with ulid");

        guard.release().expect("release ok");
        assert!(!entry.exists(), "lock file should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn acquire_rejects_when_record_is_live() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let owner = ProcessId::generate();
        let owner_dir = proc_root.join(owner.to_string());
        ensure_dir_with_mode(&owner_dir, 0o700).unwrap();
        std::fs::write(owner_dir.join("status"), b"running\n").unwrap();
        let lock_path = locks_path.join("dup");
        std::fs::write(
            &lock_path,
            format!("{}\n{}\n", owner, Utc::now().to_rfc3339()),
        )
        .unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = acquire(
            locks_fd.as_fd(),
            &locks_path,
            &proc_root,
            "dup",
            ProcessId::generate(),
        )
        .expect_err("must reject");
        assert!(matches!(err, RegistryError::AlreadyExists));
    }

    #[cfg(unix)]
    #[test]
    fn acquire_recovers_when_holder_terminated() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let dead = ProcessId::generate();
        let dead_dir = proc_root.join(dead.to_string());
        ensure_dir_with_mode(&dead_dir, 0o700).unwrap();
        std::fs::write(dead_dir.join("status"), b"failed\n").unwrap();
        let lock_path = locks_path.join("recover");
        std::fs::write(
            &lock_path,
            format!("{}\n{}\n", dead, Utc::now().to_rfc3339()),
        )
        .unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let new_owner = ProcessId::generate();
        let g = acquire(
            locks_fd.as_fd(),
            &locks_path,
            &proc_root,
            "recover",
            new_owner,
        )
        .expect("recovery acquire");
        let body = std::fs::read_to_string(locks_path.join("recover")).unwrap();
        assert!(
            body.starts_with(&new_owner.to_string()),
            "body must be rewritten to new owner: {body}"
        );
        g.release().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn acquire_recovers_when_record_missing() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let ghost = ProcessId::generate();
        let lock_path = locks_path.join("ghost");
        std::fs::write(
            &lock_path,
            format!("{}\n{}\n", ghost, Utc::now().to_rfc3339()),
        )
        .unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let me = ProcessId::generate();
        let g = acquire(locks_fd.as_fd(), &locks_path, &proc_root, "ghost", me)
            .expect("missing-record recovery");
        g.release().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_body_within_grace_returns_corrupt_lock() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let lock_path = locks_path.join("corrupt");
        std::fs::write(&lock_path, b"not-a-ulid\nnot-a-ts\n").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = acquire(
            locks_fd.as_fd(),
            &locks_path,
            &proc_root,
            "corrupt",
            ProcessId::generate(),
        )
        .expect_err("must reject");
        assert!(matches!(err, RegistryError::CorruptLock));
    }

    #[cfg(unix)]
    #[test]
    fn release_unlinks_lock_file() {
        let (_tmp, proc_root, locks_path, locks_fd) = setup_root();
        let g = acquire(
            locks_fd.as_fd(),
            &locks_path,
            &proc_root,
            "rel",
            ProcessId::generate(),
        )
        .unwrap();
        assert!(locks_path.join("rel").exists());
        g.release().unwrap();
        assert!(!locks_path.join("rel").exists());
    }
}
