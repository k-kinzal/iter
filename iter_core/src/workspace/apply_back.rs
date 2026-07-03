//! [`ApplyBackMode`] — strategy for reconciling a workspace's temp tree
//! back into its base directory.
//!
//! Used by both [`CloneWorkspace`](crate::workspace::CloneWorkspace) and
//! [`SandboxWorkspace`](crate::workspace::SandboxWorkspace); lives at the
//! workspace root so the two share a single definition (and a single set
//! of semantics).

/// Strategy used on workspace teardown to reconcile the temp copy back
/// into the base directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyBackMode {
    /// Rsync-style reconciliation.
    ///
    /// New or modified files in the temp directory are copied back to the
    /// base; files that existed in the base but no longer exist in the temp
    /// are deleted. Excluded directories in the base are left untouched.
    Sync,
    /// Never apply anything back.
    ///
    /// The temp directory is dropped on teardown, giving the agent a
    /// purely ephemeral scratch space.
    Discard,
    /// Conservative merge.
    ///
    /// New and modified files are copied back to the base (mtime
    /// comparison; temp must be strictly newer to overwrite), but nothing
    /// is deleted. Useful when the caller intends to review or further
    /// process the result of the agent's work and does not want an
    /// accidental rm-in-temp to delete files in the base.
    ///
    /// **Depends on `preserve_mtime`.** The mtime comparison is only
    /// meaningful when the temp tree carries the source files' real mtimes.
    /// With `preserve_mtime = false`, every clone-time copy is stamped with
    /// a single clone timestamp, so an *untouched* temp file compares as
    /// strictly newer than its base counterpart and is copied back
    /// wholesale — degrading `Merge` into an indiscriminate copy-everything
    /// (still non-deleting, so no data loss, but the mtime gate is a no-op).
    /// Pair `Merge` with `preserve_mtime = true`; the workspace logs a
    /// warning when it sees the incoherent combination.
    Merge,
}
