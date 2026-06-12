//! Construction-time settings for [`CloneWorkspace`](super::CloneWorkspace).

use crate::workspace::apply_back::ApplyBackMode;
use crate::workspace::mirror::ApplyBackFilter;

/// Project-shaped settings for a [`CloneWorkspace`](super::CloneWorkspace).
///
/// Every field is required — there is no `Default` impl. The project must
/// spell out its policy explicitly because iter has no honest default for
/// any of them:
///
/// - `excludes` / `includes` are filesystem-layout decisions that vary per
///   language, per build tool, per monorepo shape.
/// - `preserve_mtime` changes what information the agent can observe about
///   the source tree's history and is therefore an exploration-strategy
///   decision.
/// - `apply_back` (and its filter pair) control whether teardown writes
///   back to the base directory and which files participate in the walk;
///   that is a policy decision about committing work.
///
/// # Two filter sets, two phases
///
/// `excludes` / `includes` apply at clone time — they decide what enters
/// the temp tree. `apply_back_excludes` / `apply_back_includes` apply at
/// teardown — they decide what propagates back to base. Workspace-level
/// excludes are automatically unioned into the apply-back filter at
/// construction time so that files never copied into the sandbox cannot
/// become deletion candidates during sync-back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneSettings {
    /// Clone-time exclude patterns. Matches paths relative to the base
    /// directory; see the [`mirror`](crate::workspace::mirror) docs for
    /// the glob dialect. Empty = no exclusions.
    pub excludes: Vec<String>,
    /// Clone-time include patterns. Empty = no overrides. Entries here
    /// win over matching entries in `excludes`.
    pub includes: Vec<String>,
    /// When `true`, destination files inherit source mtimes verbatim; when
    /// `false`, copies are stamped with the clone time.
    pub preserve_mtime: bool,
    /// Reconciliation strategy used on teardown.
    pub apply_back: ApplyBackMode,
    /// Apply-back-time exclude patterns. Same glob dialect as `excludes`,
    /// matched relative to the workspace root. Empty = no exclusions.
    pub apply_back_excludes: Vec<String>,
    /// Apply-back-time include patterns. Empty = no restriction. When
    /// non-empty this acts as a whitelist: only matching paths participate
    /// in the apply-back walk (unlike clone-time `includes`, which only
    /// rescue otherwise-excluded paths).
    pub apply_back_includes: Vec<String>,
}

impl CloneSettings {
    /// Build the [`ApplyBackFilter`] with workspace-level excludes enforced
    /// unconditionally — independent of the user's apply-back includes or
    /// negation patterns.
    pub(crate) fn apply_back_filter(&self) -> Result<ApplyBackFilter, globset::Error> {
        ApplyBackFilter::compile_with_workspace_excludes(
            &self.apply_back_excludes,
            &self.apply_back_includes,
            &self.excludes,
        )
    }
}
