//! Outcome of the rollback `Failed` write performed by
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

/// Outcome of the rollback `Failed` write performed inside the locked
/// critical section. See module-level docs for the four shapes.
#[derive(Debug)]
#[non_exhaustive]
pub enum SecondaryStatusWriteOutcome {
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

impl SecondaryStatusWriteOutcome {
    /// Combine the result of `write_status_in_place(Failed)` with the result
    /// of the subsequent `fsync` into a single observable outcome.
    #[must_use]
    pub fn from_write_and_fsync(
        write: io::Result<()>,
        fsync: io::Result<()>,
    ) -> SecondaryStatusWriteOutcome {
        match (write, fsync) {
            (Ok(()), Ok(())) => SecondaryStatusWriteOutcome::Wrote,
            (Ok(()), Err(source)) => SecondaryStatusWriteOutcome::WroteButFsyncFailed { source },
            (Err(source), Ok(())) => SecondaryStatusWriteOutcome::WriteFailed { source },
            (Err(write), Err(fsync)) => SecondaryStatusWriteOutcome::BothFailed { write, fsync },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_outcome_combinations_match() {
        let oks = SecondaryStatusWriteOutcome::from_write_and_fsync(Ok(()), Ok(()));
        assert!(matches!(oks, SecondaryStatusWriteOutcome::Wrote));

        let fsync_only = SecondaryStatusWriteOutcome::from_write_and_fsync(
            Ok(()),
            Err(io::Error::other("nope")),
        );
        assert!(matches!(
            fsync_only,
            SecondaryStatusWriteOutcome::WroteButFsyncFailed { .. }
        ));

        let write_only = SecondaryStatusWriteOutcome::from_write_and_fsync(
            Err(io::Error::other("nope")),
            Ok(()),
        );
        assert!(matches!(
            write_only,
            SecondaryStatusWriteOutcome::WriteFailed { .. }
        ));

        let both = SecondaryStatusWriteOutcome::from_write_and_fsync(
            Err(io::Error::other("w")),
            Err(io::Error::other("f")),
        );
        assert!(matches!(
            both,
            SecondaryStatusWriteOutcome::BothFailed { .. }
        ));
    }
}
