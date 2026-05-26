//! Filesystem mirroring primitives shared by
//! [`CloneWorkspace`](crate::workspace::CloneWorkspace) and
//! [`SandboxWorkspace`](crate::workspace::SandboxWorkspace).
//!
//! A *mirror* is the conceptual pairing of (base directory, temp
//! directory) with two operations:
//!
//! 1. **Materialise** — recursively copy the base tree into a fresh temp
//!    directory, honouring a clone-time [`CloneFilter`] and a
//!    `preserve_mtime` toggle.
//! 2. **Reconcile** — apply the temp tree's changes back to the base tree
//!    ([`sync_back`](Mirror::sync_back) rsync-style, or
//!    [`merge_back`](Mirror::merge_back) conservatively with no deletions)
//!    or discard them outright. The reconcile walk uses a separate
//!    [`ApplyBackFilter`], so a path can be visible to the agent at clone
//!    time but blocked from propagating back on teardown.
//!
//! The public surface is deliberately narrow: [`Mirror`], [`CloneFilter`],
//! and [`ApplyBackFilter`]. Individual primitives (copy, enumerate, prune,
//! mtime) live in submodules gated at `pub(crate)`.

pub(crate) mod enumerate;
pub(crate) mod filter;
pub(crate) mod materialize;
pub(crate) mod mtime;
pub(crate) mod prune;
pub(crate) mod reconcile;

use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub(crate) use filter::{ApplyBackFilter, CloneFilter};

/// An on-disk mirror of a base directory in a fresh
/// [`tempfile::TempDir`], with the original (`base`) kept for later
/// reconciliation.
///
/// The mirror owns its [`TempDir`] so the temp tree is removed
/// deterministically when the mirror is dropped or closed — both
/// [`CloneWorkspace`](crate::workspace::CloneWorkspace) and
/// [`SandboxWorkspace`](crate::workspace::SandboxWorkspace) rely on this
/// to keep teardown ordering coherent regardless of which workspace
/// invokes them.
///
/// The [`CloneFilter`] is consumed during construction (it only affects
/// the materialisation walk); the [`ApplyBackFilter`] is stored for use
/// by [`sync_back`](Self::sync_back) / [`merge_back`](Self::merge_back).
#[derive(Debug)]
pub(crate) struct Mirror {
    base: PathBuf,
    temp: TempDir,
    temp_path: PathBuf,
    apply_back_filter: ApplyBackFilter,
}

impl Mirror {
    /// Materialise a mirror: create a fresh temp directory and copy the
    /// contents of `base` into it, honouring `clone_filter` and
    /// `preserve_mtime`. The supplied `apply_back_filter` is stored for
    /// later use by [`sync_back`](Self::sync_back) /
    /// [`merge_back`](Self::merge_back).
    ///
    /// Spawns the blocking [`TempDir::new`] on the Tokio blocking pool so
    /// the reactor thread is not stalled.
    pub(crate) async fn materialize(
        base: PathBuf,
        clone_filter: &CloneFilter,
        apply_back_filter: ApplyBackFilter,
        preserve_mtime: bool,
    ) -> io::Result<Self> {
        let temp = tokio::task::spawn_blocking(TempDir::new)
            .await
            .map_err(io::Error::other)??;
        let temp_path = temp.path().to_path_buf();

        materialize::copy_dir_recursive(&base, &temp_path, clone_filter, preserve_mtime).await?;

        Ok(Self {
            base,
            temp,
            temp_path,
            apply_back_filter,
        })
    }

    /// Path to the temp-side mirror of the base directory — this is the
    /// directory the agent actually operates against.
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.temp_path
    }

    /// Rsync-style apply-back: the temp tree becomes the source of truth
    /// on the base side. Files missing from the temp tree are deleted on
    /// the base side; empty directories left behind are pruned. The
    /// stored [`ApplyBackFilter`] masks files on both sides.
    pub(crate) async fn sync_back(&self) -> io::Result<()> {
        reconcile::sync_back_impl(&self.base, &self.temp_path, &self.apply_back_filter).await
    }

    /// Conservative apply-back: files from the temp tree are written to
    /// the base side only when the temp copy's mtime is strictly newer
    /// than the base copy's mtime. Nothing is deleted. The stored
    /// [`ApplyBackFilter`] masks files from both walks.
    pub(crate) async fn merge_back(&self) -> io::Result<()> {
        reconcile::merge_back_impl(&self.base, &self.temp_path, &self.apply_back_filter).await
    }

    /// Close the mirror, deleting its temp directory and surfacing any
    /// pending I/O errors that [`Drop`] on [`TempDir`] would silently
    /// swallow.
    ///
    /// Runs the blocking [`TempDir::close`] on the Tokio blocking pool.
    pub(crate) async fn close(self) -> io::Result<()> {
        let Self { temp, .. } = self;
        tokio::task::spawn_blocking(move || temp.close())
            .await
            .map_err(io::Error::other)?
    }

    /// Best-effort close: attempt [`close`](Self::close), and if it
    /// fails, fall back to a direct [`remove_dir_all`](tokio::fs::remove_dir_all).
    /// Logs warnings on failure but never returns an error — the temp
    /// directory is either gone or irremovable, and either way the
    /// caller should continue teardown.
    pub(crate) async fn close_best_effort(self) {
        let temp_path = self.temp_path.clone();
        if let Err(e) = self.close().await {
            tracing::warn!(
                path = %temp_path.display(),
                error = %e,
                "mirror close failed; removing temp directory directly",
            );
            match tokio::fs::remove_dir_all(&temp_path).await {
                Ok(()) => {}
                Err(e2) if e2.kind() == io::ErrorKind::NotFound => {}
                Err(e2) => {
                    tracing::warn!(
                        path = %temp_path.display(),
                        error = %e2,
                        "fallback temp directory removal also failed",
                    );
                }
            }
        }
    }
}
