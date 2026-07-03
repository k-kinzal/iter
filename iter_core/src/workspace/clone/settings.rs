//! Construction-time settings for [`CloneWorkspace`](super::CloneWorkspace).

use std::path::Path;

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
/// teardown — they decide what propagates back to base. The clone-time
/// `excludes` *and* `includes` are both woven into the apply-back filter as
/// a workspace gate at construction time, reproducing the clone filter's
/// exclusion decision exactly: a path the clone filter dropped (never
/// materialised into the sandbox) cannot become a deletion candidate during
/// sync-back, while a path a clone-time include rescued into the sandbox is
/// free to propagate back.
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
    /// `false`, copies are stamped with a single clone timestamp.
    ///
    /// Note the interaction with [`ApplyBackMode::Merge`]: Merge's
    /// strictly-newer mtime gate is only meaningful when the temp tree
    /// carries real source mtimes. With `preserve_mtime = false`, every copy
    /// shares one timestamp, so untouched files compare as newer and are
    /// copied back wholesale — pair `Merge` with `preserve_mtime = true`.
    pub preserve_mtime: bool,
    /// Reconciliation strategy used on teardown.
    ///
    /// [`ApplyBackMode::Merge`] additionally depends on `preserve_mtime`
    /// being `true` for its mtime gate to function; see that variant and the
    /// `preserve_mtime` field.
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
    /// Build the [`ApplyBackFilter`] with a workspace gate enforced
    /// unconditionally — independent of the user's apply-back includes or
    /// negation patterns. The gate mirrors the clone filter (both
    /// `excludes` and `includes`) so a clone-time include rescue is honoured
    /// at teardown rather than silently dropped.
    pub(crate) fn apply_back_filter(&self) -> Result<ApplyBackFilter, globset::Error> {
        ApplyBackFilter::compile_with_workspace_excludes(
            &self.apply_back_excludes,
            &self.apply_back_includes,
            &self.excludes,
            &self.includes,
        )
    }

    /// Warn when [`ApplyBackMode::Merge`] is paired with
    /// `preserve_mtime = false` — a combination in which Merge's
    /// strictly-newer mtime gate is defeated (every clone-time copy shares
    /// one timestamp, so untouched files compare as newer and are copied
    /// back wholesale). The result is non-deleting, so there is no data
    /// loss; iter warns rather than rejects because the combination is
    /// merely incoherent, not unsafe. Called once per workspace at
    /// construction time. `workspace` names the workspace kind and `base` its
    /// root path, so the record is attributable across concurrent runners.
    pub(crate) fn warn_if_merge_gate_defeated(&self, workspace: &str, base: &Path) {
        if self.apply_back == ApplyBackMode::Merge && !self.preserve_mtime {
            tracing::warn!(
                workspace,
                base = %base.display(),
                "apply_back = Merge with preserve_mtime = false: Merge's mtime gate is \
                 defeated (all clone-time copies share one timestamp), so untouched files \
                 are copied back wholesale; pair Merge with preserve_mtime = true",
            );
        }
    }
}
