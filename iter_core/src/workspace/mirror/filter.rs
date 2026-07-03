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
//! * **Rescue is leaf-scoped; any exclude that matches a directory entry
//!   prunes its whole subtree.** The clone walk (`copy_dir_recursive`) skips
//!   an excluded directory and never descends into it, so a child negation or
//!   include cannot reach inside a directory whose *own path* an exclude
//!   matches — whether by bare name (`vendor`) or anchored path (`a/b`).
//!   `["vendor", "!vendor/keep"]` drops all of `vendor` and the negation is
//!   moot. The apply-back [`ApplyBackFilter`] workspace gate mirrors this — a
//!   child of a clone-excluded directory stays masked — so the rule is uniform
//!   across both phases. To drop a directory's *contents* while keeping one
//!   child, match the contents but not the directory entry itself, then negate
//!   the child — `["vendor/*", "!vendor/keep"]` — which the walk descends into
//!   normally (`vendor/*` never matches the bare `vendor` entry).

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
    has_negations: bool,
}

impl GlobPair {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            excludes: GlobSet::empty(),
            negations: GlobSet::empty(),
            includes: GlobSet::empty(),
            has_includes: false,
            has_negations: false,
        }
    }

    fn compile(excludes: &[String], includes: &[String]) -> Result<Self, globset::Error> {
        let (negated, positive): (Vec<_>, Vec<_>) =
            excludes.iter().partition(|p| p.starts_with('!'));
        let neg_patterns: Vec<String> = negated
            .iter()
            .map(|p| {
                p.strip_prefix('!')
                    .expect("negated exclude patterns are selected by starts_with('!')")
                    .to_string()
            })
            .collect();
        let pos_patterns: Vec<String> = positive.into_iter().cloned().collect();
        Ok(Self {
            excludes: compile_patterns(&pos_patterns)?,
            negations: compile_patterns(&neg_patterns)?,
            includes: compile_patterns(includes)?,
            has_includes: !includes.is_empty(),
            has_negations: !neg_patterns.is_empty(),
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
/// `true`. Two layers combine:
///
/// 1. A **workspace gate** built from the clone-time [`CloneFilter`]'s
///    exclude/include lists. It masks exactly the paths the clone-time walk
///    would *not* have materialised — a path is gated iff some
///    ancestor-or-self prefix is excluded by the clone filter, mirroring the
///    walk's directory-granular pruning. This lets a genuine file-pattern
///    include rescue (whose leaf has no excluded ancestor) propagate back,
///    while a child of an excluded directory — never copied into the sandbox
///    — stays masked and can never become a deletion candidate.
/// 2. The apply-back `excludes`/`includes` pair. When `includes` is
///    non-empty it acts as a whitelist — only matching paths pass.
///    Otherwise `excludes` applies, with `!pattern` negation support.
///
/// [`is_excluded`]: ApplyBackFilter::is_excluded
#[derive(Debug, Clone)]
pub(crate) struct ApplyBackFilter {
    workspace_gate: GlobPair,
    inner: GlobPair,
}

impl ApplyBackFilter {
    #[cfg(test)]
    pub(crate) fn compile(
        excludes: &[String],
        includes: &[String],
    ) -> Result<Self, globset::Error> {
        Ok(Self {
            workspace_gate: GlobPair::empty(),
            inner: GlobPair::compile(excludes, includes)?,
        })
    }

    /// Compile with a workspace-level gate that is enforced unconditionally
    /// — before the apply-back includes-whitelist or negation logic. The
    /// gate is built from the *same* exclude/include lists the clone-time
    /// [`CloneFilter`] uses, and [`is_excluded`](Self::is_excluded) applies
    /// it prefix-wise so it reproduces the clone-time *walk*: a path the walk
    /// would not have materialised (some ancestor-or-self prefix is excluded)
    /// cannot become an apply-back candidate, while a path a clone-time
    /// include genuinely rescued into the sandbox is free to propagate back.
    pub(crate) fn compile_with_workspace_excludes(
        excludes: &[String],
        includes: &[String],
        workspace_excludes: &[String],
        workspace_includes: &[String],
    ) -> Result<Self, globset::Error> {
        Ok(Self {
            workspace_gate: GlobPair::compile(workspace_excludes, workspace_includes)?,
            inner: GlobPair::compile(excludes, includes)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            workspace_gate: GlobPair::empty(),
            inner: GlobPair::empty(),
        }
    }

    /// `true` when the clone-time walk would NOT have materialised any path at
    /// or below `rel` — i.e. some ancestor-or-self prefix is excluded by the
    /// workspace gate.
    ///
    /// The clone walk (`copy_dir_recursive`) prunes at directory granularity —
    /// it skips an excluded entry and never descends — so a path is
    /// materialised iff *no* ancestor-or-self prefix is excluded by the clone
    /// filter. Testing every prefix (not just the leaf) is what keeps a
    /// clone-time include that names a *child of an excluded directory* from
    /// wrongly un-masking that child: the child was never copied in, so it
    /// stays gated. A genuine file-pattern rescue (e.g. `*.secret` excluded,
    /// `keep.secret` included) has no excluded ancestor, so it is un-gated.
    ///
    /// Shared by [`is_excluded`](Self::is_excluded) (a gated leaf can neither
    /// be copied back nor deleted from base) and
    /// [`should_descend`](Self::should_descend) (a gated directory was never
    /// materialised, so the apply-back walk must not enter it).
    fn workspace_gate_masks(&self, rel: &Path) -> bool {
        rel.ancestors()
            .any(|prefix| !prefix.as_os_str().is_empty() && self.workspace_gate.is_excluded(prefix))
    }

    /// Whether the leaf `rel` participates in apply-back (copy back / delete
    /// from base). This is the file-level masking decision — distinct from the
    /// directory-descent decision in [`should_descend`](Self::should_descend).
    pub(crate) fn is_excluded(&self, rel: &Path) -> bool {
        // Workspace gate: mask exactly the paths the clone-time walk would NOT
        // have materialised, so a file that never entered the sandbox can
        // neither be copied back nor deleted from base.
        if self.workspace_gate_masks(rel) {
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

    /// Whether the apply-back walk should recurse into directory `rel`.
    ///
    /// Directory descent and leaf masking are *different* questions, and
    /// conflating them (as a single `is_excluded` check for both would)
    /// silently defeats the "exclude a directory except one file" intent.
    /// The leaf-level [`is_excluded`](Self::is_excluded) decides whether an
    /// individual file participates; this decides whether the walk enters a
    /// directory at all.
    ///
    /// They diverge because apply-back's own `!negation` and whitelist
    /// `includes` can re-include a child *inside* a directory the leaf filter
    /// excludes. `apply_back_excludes = ["vendor", "!vendor/keep"]` excludes
    /// the `vendor` directory at the leaf level, yet the walk must still
    /// descend into it to reach `vendor/keep` — pruning at `vendor` (which the
    /// leaf `is_excluded` alone would force) would leave the negation dead.
    /// The same holds for a whitelist include naming a child of an otherwise
    /// unmatched directory.
    ///
    /// The clone-derived workspace gate is different: it reflects a physical
    /// fact (the subtree was never materialised), so a gated directory is
    /// skipped unconditionally. And a plain directory exclude carrying neither
    /// a negation nor a whitelist can be pruned wholesale — nothing inside it
    /// could ever be re-included — preserving fast subtree pruning for the
    /// common case.
    pub(crate) fn should_descend(&self, rel: &Path) -> bool {
        if self.workspace_gate_masks(rel) {
            return false;
        }
        // Enter an apply-back-excluded directory only when a negation or a
        // whitelist include could still re-include a child within it.
        if self.inner.is_excluded(rel) {
            return self.inner.has_negations || self.inner.has_includes;
        }
        true
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
            &[],
        )
        .expect("test patterns must compile");
        assert!(f.is_excluded(Path::new(".git/HEAD")));
        assert!(!f.is_excluded(Path::new("src/main.rs")));
    }

    #[test]
    fn workspace_gate_honors_clone_includes_rescue() {
        // A genuinely clone-reachable rescue: the clone filter excludes the
        // file pattern `*.secret` but a clone-time include rescues
        // `keep.secret`. Because no *directory* is excluded, the clone walk
        // descends normally and materialises `keep.secret`, so at teardown the
        // apply-back gate must let it propagate back — even under a `**`
        // apply-back whitelist — while its excluded sibling stays gated.
        let f = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &["**".to_owned()],
            &["*.secret".to_owned()],
            &["keep.secret".to_owned()],
        )
        .expect("test patterns must compile");
        assert!(
            !f.is_excluded(Path::new("keep.secret")),
            "a clone-time include rescue must survive the apply-back gate",
        );
        assert!(
            f.is_excluded(Path::new("drop.secret")),
            "a sibling the clone filter excluded was never copied in and stays gated",
        );
        assert!(!f.is_excluded(Path::new("src/main.rs")));
    }

    #[test]
    fn workspace_gate_masks_child_of_excluded_dir_despite_include() {
        // The clone filter excludes the *directory* `vendor` and a clone-time
        // include names a child, `vendor/keep`. The clone walk prunes at
        // `vendor/` and never descends, so `vendor/keep` is NOT materialised —
        // an include cannot rescue a child of an excluded directory. The
        // apply-back gate must therefore keep the whole subtree masked so a
        // base file that never entered the sandbox is never deleted, even
        // under a `**` whitelist.
        let f = ApplyBackFilter::compile_with_workspace_excludes(
            &[],
            &["**".to_owned()],
            &["vendor".to_owned()],
            &["vendor/keep".to_owned()],
        )
        .expect("test patterns must compile");
        assert!(
            f.is_excluded(Path::new("vendor/keep")),
            "an include cannot un-mask a child of an excluded directory",
        );
        assert!(f.is_excluded(Path::new("vendor/other")));
        assert!(!f.is_excluded(Path::new("src/main.rs")));
    }

    #[test]
    fn should_descend_prunes_plain_excluded_dir() {
        // A plain directory exclude with neither a negation nor a whitelist can
        // never re-include anything inside it — the walk prunes it wholesale,
        // preserving fast subtree skipping for the common case.
        let f = apply_f(&["vendor"], &[]);
        assert!(!f.should_descend(Path::new("vendor")));
        assert!(f.should_descend(Path::new("src")));
    }

    #[test]
    fn should_descend_enters_excluded_dir_with_negation() {
        // "exclude `vendor` except `vendor/keep`": the leaf filter excludes the
        // directory, yet the walk must still descend to reach the negated child
        // — otherwise the negation is silently dead. The leaf decision then
        // rescues the child while masking its siblings.
        let f = apply_f(&["vendor", "!vendor/keep"], &[]);
        assert!(
            f.should_descend(Path::new("vendor")),
            "must descend into an excluded dir to reach its negation-rescued child",
        );
        assert!(!f.is_excluded(Path::new("vendor/keep")));
        assert!(f.is_excluded(Path::new("vendor/other")));
    }

    #[test]
    fn should_descend_enters_dir_under_whitelist() {
        // Whitelist mode: any directory may hold a whitelisted child, so the
        // walk descends everywhere the gate allows and lets the leaf filter
        // keep only matching paths.
        let f = apply_f(&[], &["vendor/keep"]);
        assert!(f.should_descend(Path::new("vendor")));
        assert!(!f.is_excluded(Path::new("vendor/keep")));
        assert!(f.is_excluded(Path::new("vendor/other")));
    }

    #[test]
    fn should_descend_skips_workspace_gated_dir_even_with_negation() {
        // The workspace gate reflects a physical fact — the clone walk never
        // materialised `vendor` — so the apply-back walk must not enter it even
        // though the apply-back excludes carry a negation. Nothing was ever
        // copied in; there is nothing to rescue.
        let f = ApplyBackFilter::compile_with_workspace_excludes(
            &["!vendor/keep".to_owned()],
            &[],
            &["vendor".to_owned()],
            &[],
        )
        .expect("test patterns must compile");
        assert!(
            !f.should_descend(Path::new("vendor")),
            "a clone-gated subtree was never materialised; do not descend",
        );
    }

    #[test]
    fn should_descend_enters_unexcluded_dir() {
        let f = apply_f(&["*.md"], &[]);
        assert!(f.should_descend(Path::new("src")));
        assert!(f.should_descend(Path::new("docs")));
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
