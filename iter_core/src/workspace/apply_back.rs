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
    Merge,
}
