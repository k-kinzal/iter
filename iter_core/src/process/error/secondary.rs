//! Result of the rollback `Failed` write performed by
//! `locked_initial_write` / `locked_adoption_write` after a primary
//! failure inside the critical section.
//!
//! Carried as a `secondary` field on every primary error variant in
//! [`super::locked_section::LockedSectionError`] that may have triggered a
//! rollback. `Wrote` means the proc record will be observed as `Failed`
//! by `iter ps`; the other shapes mean the file is in a best-effort
//! intermediate state and `refresh_status` will reconcile it once the
//! bootstrap grace elapses.

use std::io;

/// Result of the rollback `Failed` write performed inside the locked
/// critical section. See module-level docs for the four shapes.
#[derive(Debug)]
#[non_exhaustive]
pub enum SecondaryStatusWriteResult {
    /// `write_status_in_place(Failed)` and the subsequent `fsync` both
    /// succeeded.
    Wrote,
    /// `write_status_in_place(Failed)` succeeded but `fsync` did not. The
    /// page cache is consistent but the change is not durable across a
    /// kernel panic.
    WroteButFsyncFailed {
        /// `fsync` error.
        source: io::Error,
    },
    /// `write_status_in_place(Failed)` itself failed (after `set_len(0)`).
    /// `read_status` will subsequently observe a corrupt body which the
    /// reconciler upgrades to Failed once the grace period elapses.
    WriteFailed {
        /// `write_all` error.
        source: io::Error,
    },
    /// Both the write and the `fsync` failed.
    BothFailed {
        /// `write_all` error.
        write: io::Error,
        /// `fsync` error.
        fsync: io::Error,
    },
}

impl SecondaryStatusWriteResult {
    /// Combine the result of `write_status_in_place(Failed)` with the result
    /// of the subsequent `fsync` into a single observable result.
    #[must_use]
    pub fn from_write_and_fsync(
        write: io::Result<()>,
        fsync: io::Result<()>,
    ) -> SecondaryStatusWriteResult {
        match (write, fsync) {
            (Ok(()), Ok(())) => SecondaryStatusWriteResult::Wrote,
            (Ok(()), Err(source)) => SecondaryStatusWriteResult::WroteButFsyncFailed { source },
            (Err(source), Ok(())) => SecondaryStatusWriteResult::WriteFailed { source },
            (Err(write), Err(fsync)) => SecondaryStatusWriteResult::BothFailed { write, fsync },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_result_combinations_match() {
        let oks = SecondaryStatusWriteResult::from_write_and_fsync(Ok(()), Ok(()));
        assert!(matches!(oks, SecondaryStatusWriteResult::Wrote));

        let fsync_only = SecondaryStatusWriteResult::from_write_and_fsync(
            Ok(()),
            Err(io::Error::other("nope")),
        );
        assert!(matches!(
            fsync_only,
            SecondaryStatusWriteResult::WroteButFsyncFailed { .. }
        ));

        let write_only = SecondaryStatusWriteResult::from_write_and_fsync(
            Err(io::Error::other("nope")),
            Ok(()),
        );
        assert!(matches!(
            write_only,
            SecondaryStatusWriteResult::WriteFailed { .. }
        ));

        let both = SecondaryStatusWriteResult::from_write_and_fsync(
            Err(io::Error::other("w")),
            Err(io::Error::other("f")),
        );
        assert!(matches!(
            both,
            SecondaryStatusWriteResult::BothFailed { .. }
        ));
    }
}
