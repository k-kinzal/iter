//! Path filtering for [`Mirror`](super::Mirror).
//!
//! Two structurally-identical types — [`CloneFilter`] and [`ApplyBackFilter`]
//! — segregate the clone-time materialisation filter from the teardown-time
//! reconciliation filter. The distinct names exist purely so the type system
//! catches accidental cross-wiring at call sites: feeding a [`CloneFilter`]
//! into [`super::reconcile`] is a compile error.
//!
//! Patterns are evaluated against paths *relative to the mirror root* using
//! [`globset`]. Semantics:
//!
//! * `**` traverses directories.
//! * `*` and `?` do **not** cross `/` (we set `literal_separator(true)`).
//! * Bare patterns (no `/`) match the basename at any depth — `node_modules`
//!   matches both `node_modules` and `vendor/node_modules`.
//! * Every pattern auto-synthesises `<P>/**` so descendants of a matched
//!   directory are also matched (avoids the "empty `target/` left behind"
//!   footgun).
//! * `excludes` supports `!pattern` negation to rescue specific paths.
//! * `includes` semantics differ per phase. At clone time they only
//!   *rescue*: an include overrides a matching exclude, and a path matching
//!   neither list always materialises. At apply-back time a non-empty
//!   `includes` is a *whitelist*: only matching paths pass.

use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Compiled exclude/include glob pair shared by both phase-specific filter
/// types.
///
/// [`is_excluded`](GlobPair::is_excluded) implements the rescue contract
/// (includes and `!` negations only override excludes); the apply-back
/// whitelist contract lives in [`ApplyBackFilter`].
#[derive(Debug, Clone)]
struct GlobPair {
    excludes: GlobSet,
    negations: GlobSet,
    includes: GlobSet,
    has_includes: bool,
}

impl GlobPair {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            excludes: GlobSet::empty(),
            negations: GlobSet::empty(),
            includes: GlobSet::empty(),
            has_includes: false,
        }
    }

    fn compile(excludes: &[String], includes: &[String]) -> Result<Self, globset::Error> {
        let (negated, positive): (Vec<_>, Vec<_>) =
            excludes.iter().partition(|p| p.starts_with('!'));
        let neg_patterns: Vec<String> = negated
            .iter()
            .map(|p| p.strip_prefix('!').unwrap().to_string())
            .collect();
        let pos_patterns: Vec<String> = positive.into_iter().cloned().collect();
        Ok(Self {
            excludes: compile_patterns(&pos_patterns)?,
            negations: compile_patterns(&neg_patterns)?,
            includes: compile_patterns(includes)?,
            has_includes: !includes.is_empty(),
        })
    }

    fn is_excluded(&self, rel: &Path) -> bool {
        if self.negations.is_match(rel) || self.includes.is_match(rel) {
            return false;
        }
        self.excludes.is_match(rel)
    }
}

/// Compile a list of user-supplied patterns into a [`GlobSet`], applying
/// iter's bare-pattern + descendant synthesis (see module docs).
fn compile_patterns(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let p = pattern.as_str();
        builder.add(make_glob(p)?);
        builder.add(make_glob(&format!("{p}/**"))?);
        if !p.contains('/') {
            builder.add(make_glob(&format!("**/{p}"))?);
            builder.add(make_glob(&format!("**/{p}/**"))?);
        }
    }
    builder.build()
}

/// Build a single [`Glob`] with `literal_separator(true)` — `*` and `?` do
/// not cross path separators, only `**` does. This gives the gitignore-ish
/// semantics the plan documents.
fn make_glob(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

/// Filter applied at clone-time when materialising files into the temp tree.
///
/// A path is dropped from the materialisation walk iff [`is_excluded`] returns
/// `true`. `excludes` applies with `!pattern` negation support. `includes`
/// only override `excludes`: a path matching neither list always
/// materialises — clone-side includes rescue, they never whitelist.
///
/// [`is_excluded`]: CloneFilter::is_excluded
#[derive(Debug, Clone)]
pub(crate) struct CloneFilter {
    inner: GlobPair,
}

impl CloneFilter {
    /// Compile the user-supplied pattern lists into a clone-time filter.
    pub(crate) fn compile(
        excludes: &[String],
        includes: &[String],
    ) -> Result<Self, globset::Error> {
        Ok(Self {
            inner: GlobPair::compile(excludes, includes)?,
        })
    }

    /// Filter that excludes nothing.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            inner: GlobPair::empty(),
        }
    }

    /// Returns `true` if `rel` (path relative to the mirror root) should be
    /// skipped during materialisation.
    pub(crate) fn is_excluded(&self, rel: &Path) -> bool {
        self.inner.is_excluded(rel)
    }
}

/// Filter applied at teardown when copying changes from the temp tree back
/// to base.
///
/// A path is dropped from the apply-back walk iff [`is_excluded`] returns
/// `true`. When `includes` is non-empty it acts as a whitelist — only matching
/// paths pass. Otherwise `excludes` applies, with `!pattern` negation support.
///
/// [`is_excluded`]: ApplyBackFilter::is_excluded
#[derive(Debug, Clone)]
pub(crate) struct ApplyBackFilter {
    workspace_excludes: GlobSet,
    inner: GlobPair,
}

impl ApplyBackFilter {
    #[cfg(test)]
    pub(crate) fn compile(
        excludes: &[String],
        includes: &[String],
    ) -> Result<Self, globset::Error> {
        Ok(Self {
            workspace_excludes: GlobSet::empty(),
            inner: GlobPair::compile(excludes, includes)?,
        })
    }

    /// Compile with a separate set of workspace-level excludes that are
    /// enforced unconditionally — before includes-whitelist or negation
    /// logic. This ensures files never copied into the sandbox cannot
    /// become deletion candidates regardless of the user's apply-back
    /// include/negation configuration.
    pub(crate) fn compile_with_workspace_excludes(
        excludes: &[String],
        includes: &[String],
        workspace_excludes: &[String],
    ) -> Result<Self, globset::Error> {
        Ok(Self {
            workspace_excludes: compile_patterns(workspace_excludes)?,
            inner: GlobPair::compile(excludes, includes)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            workspace_excludes: GlobSet::empty(),
            inner: GlobPair::empty(),
        }
    }

    pub(crate) fn is_excluded(&self, rel: &Path) -> bool {
        if self.workspace_excludes.is_match(rel) {
            return true;
        }
        // The whitelist contract is apply-back-only; clone-side includes
        // merely rescue (see GlobPair::is_excluded). While the whitelist
        // is active, `excludes` — including `!` negations — are moot.
        if self.inner.has_includes {
            return !self.inner.includes.is_match(rel);
        }
        self.inner.is_excluded(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clone_f(excludes: &[&str], includes: &[&str]) -> CloneFilter {
        CloneFilter::compile(
            &excludes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            &includes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        )
        .expect("test patterns must compile")
    }

    fn apply_f(excludes: &[&str], includes: &[&str]) -> ApplyBackFilter {
        ApplyBackFilter::compile(
            &excludes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            &includes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        )
        .expect("test patterns must compile")
    }

    #[test]
    fn empty_filter_excludes_nothing() {
        let f = CloneFilter::empty();
        assert!(!f.is_excluded(Path::new("a.rs")));
        assert!(!f.is_excluded(Path::new("nested/b.rs")));
        assert!(!f.is_excluded(Path::new("")));
    }

    #[test]
    fn bare_basename_matches_at_any_depth() {
        let f = clone_f(&["node_modules"], &[]);
        assert!(f.is_excluded(Path::new("node_modules")));
        assert!(f.is_excluded(Path::new("node_modules/x.json")));
        assert!(f.is_excluded(Path::new("vendor/node_modules")));
        assert!(f.is_excluded(Path::new("vendor/node_modules/x.json")));
        assert!(!f.is_excluded(Path::new("src/main.rs")));
        assert!(!f.is_excluded(Path::new("node_modules_backup")));
    }

    #[test]
    fn bare_glob_matches_basename_anywhere() {
        let f = clone_f(&["*.md"], &[]);
        assert!(f.is_excluded(Path::new("foo.md")));
        assert!(f.is_excluded(Path::new("docs/foo.md")));
        assert!(f.is_excluded(Path::new("docs/sub/foo.md")));
        assert!(!f.is_excluded(Path::new("foo.markdown")));
        assert!(!f.is_excluded(Path::new("foo.txt")));
    }

    #[test]
    fn anchored_path_does_not_match_at_other_depth() {
        let f = clone_f(&["docs/**"], &[]);
        assert!(f.is_excluded(Path::new("docs/a")));
        assert!(f.is_excluded(Path::new("docs/sub/b.md")));
        assert!(!f.is_excluded(Path::new("other/docs/a")));
    }

    #[test]
    fn clone_includes_rescue_excluded_paths() {
        let f = clone_f(&["hidden", "drop"], &["hidden"]);
        assert!(!f.is_excluded(Path::new("hidden/value.txt")));
        assert!(f.is_excluded(Path::new("drop/me.txt")));
        assert!(!f.is_excluded(Path::new("keep.txt")));
    }

    #[test]
    fn clone_includes_never_whitelist() {
        let f = clone_f(&[], &["*.rs"]);
        assert!(!f.is_excluded(Path::new("main.rs")));
        assert!(!f.is_excluded(Path::new("README.md")));
        assert!(!f.is_excluded(Path::new("Cargo.toml")));
    }

    #[test]
    fn apply_back_includes_act_as_whitelist() {
        let f = apply_f(&[], &["*.rs"]);
        assert!(!f.is_excluded(Path::new("main.rs")));
        assert!(!f.is_excluded(Path::new("src/lib.rs")));
        assert!(f.is_excluded(Path::new("README.md")));
        assert!(f.is_excluded(Path::new("Cargo.toml")));
    }

    #[test]
    fn apply_back_whitelist_ignores_excludes() {
        let f = apply_f(&["*.rs"], &["*.rs"]);
        assert!(!f.is_excluded(Path::new("main.rs")));
        assert!(f.is_excluded(Path::new("README.md")));
    }

    #[test]
    fn apply_back_whitelist_ignores_negations() {
        let f = apply_f(&["*.md", "!docs/config/**"], &["*.rs"]);
        assert!(!f.is_excluded(Path::new("main.rs")));
        assert!(
            f.is_excluded(Path::new("docs/config/spec.md")),
            "a `!` negation cannot punch through an active whitelist",
        );
    }

    #[test]
    fn workspace_excludes_override_whitelist() {
        let f = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &["**".to_owned()],
            &[".git".to_owned()],
        )
        .expect("test patterns must compile");
        assert!(f.is_excluded(Path::new(".git/HEAD")));
        assert!(!f.is_excluded(Path::new("src/main.rs")));
    }

    #[test]
    fn negation_rescues_from_excludes() {
        let f = clone_f(&["*.md", "!docs/config/**"], &[]);
        assert!(f.is_excluded(Path::new("README.md")));
        assert!(f.is_excluded(Path::new("docs/guide.md")));
        assert!(!f.is_excluded(Path::new("docs/config/spec.md")));
        assert!(!f.is_excluded(Path::new("docs/config/deep/ref.md")));
        assert!(!f.is_excluded(Path::new("main.rs")));
    }

    #[test]
    fn negation_in_apply_back() {
        let f = apply_f(&["*.md", "!docs/config/**"], &[]);
        assert!(f.is_excluded(Path::new("README.md")));
        assert!(!f.is_excluded(Path::new("docs/config/spec.md")));
        assert!(!f.is_excluded(Path::new("src/main.rs")));
    }

    #[test]
    fn directory_match_implies_descendants() {
        let f = clone_f(&["target"], &[]);
        assert!(f.is_excluded(Path::new("target")));
        assert!(f.is_excluded(Path::new("target/debug")));
        assert!(f.is_excluded(Path::new("target/debug/x.rs")));
    }

    #[test]
    fn slash_pattern_descendants_covered_by_synthesis() {
        // `nope/*` alone cannot cross `/` (literal_separator(true)), but
        // `compile_patterns` auto-synthesises `nope/**` so descendants at
        // arbitrary depth are still excluded.
        let f = clone_f(&["nope/*"], &[]);
        assert!(f.is_excluded(Path::new("nope/a.txt")));
        assert!(f.is_excluded(Path::new("nope/a/b.txt")));
        assert!(!f.is_excluded(Path::new("a.txt")));
    }

    #[test]
    fn star_does_not_cross_separator_without_synthesis() {
        // Confirm the underlying matcher really does treat `*` as
        // separator-bound: a single `nope/*` Glob built directly does not
        // match `nope/a/b.txt`. The `compile_patterns` helper above is what
        // adds the descendant coverage.
        let g = make_glob("nope/*").expect("valid glob");
        let mut b = GlobSetBuilder::new();
        b.add(g);
        let set = b.build().expect("build");
        assert!(set.is_match(Path::new("nope/a.txt")));
        assert!(!set.is_match(Path::new("nope/a/b.txt")));
    }

    #[test]
    fn apply_back_filter_has_same_semantics() {
        let f = apply_f(&["*.md"], &[]);
        assert!(f.is_excluded(Path::new("foo.md")));
        assert!(f.is_excluded(Path::new("docs/foo.md")));
        assert!(!f.is_excluded(Path::new("foo.txt")));
    }

    #[test]
    fn invalid_pattern_returns_error() {
        let err = CloneFilter::compile(&["[unclosed".to_owned()], &[]);
        assert!(err.is_err(), "expected compile error for malformed glob");
    }

    #[test]
    fn empty_filters_match_nothing() {
        let f = ApplyBackFilter::empty();
        assert!(!f.is_excluded(Path::new("anything")));
        assert!(!f.is_excluded(Path::new("nested/path")));
    }
}
