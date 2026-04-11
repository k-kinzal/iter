//! Path filtering helpers for the watch trigger.
//!
//! Patterns are gitignore-style globs evaluated against paths relative to the
//! watch root via [`globset`]. `**` traverses directories. An empty include
//! list is treated as "match anything"; matching exclude patterns always wins
//! over includes.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Compile `patterns` into a [`GlobSet`]. An empty input yields an empty set
/// (which matches nothing — callers use [`path_matches`]'s `include_empty`
/// flag to express "everything matches").
pub(super) fn compile_globset(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    builder.build()
}

/// Test whether `rel` (relative to the watch root) survives the include /
/// exclude filters.
///
/// - `exclude` matches always reject.
/// - `include_empty = true` accepts everything that wasn't excluded.
/// - Otherwise the path must hit at least one `include` glob.
pub(super) fn path_matches(
    rel: &Path,
    include_empty: bool,
    include: &GlobSet,
    exclude: &GlobSet,
) -> bool {
    if exclude.is_match(rel) {
        return false;
    }
    if include_empty {
        return true;
    }
    include.is_match(rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn set(patterns: &[&str]) -> GlobSet {
        compile_globset(&patterns.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn empty_include_matches_all() {
        let inc = set(&[]);
        let exc = set(&[]);
        assert!(path_matches(Path::new("a.txt"), true, &inc, &exc));
        assert!(path_matches(Path::new("nested/dir/file"), true, &inc, &exc));
    }

    #[test]
    fn include_only_extension() {
        let inc = set(&["*.rs"]);
        let exc = set(&[]);
        // GlobSet's default Glob lets `*` cross path separators, so nested
        // files still match — gitignore-style.
        assert!(path_matches(Path::new("foo.rs"), false, &inc, &exc));
        assert!(path_matches(Path::new("src/foo.rs"), false, &inc, &exc));
        assert!(!path_matches(Path::new("foo.md"), false, &inc, &exc));
    }

    #[test]
    fn include_double_star_directory() {
        let inc = set(&["**/*.jsonl"]);
        let exc = set(&[]);
        assert!(path_matches(Path::new("a.jsonl"), false, &inc, &exc));
        assert!(path_matches(
            Path::new("deep/nested/log.jsonl"),
            false,
            &inc,
            &exc,
        ));
        assert!(!path_matches(Path::new("a.json"), false, &inc, &exc));
    }

    #[test]
    fn exclude_directory_prefix() {
        let inc = set(&[]);
        let exc = set(&["skip/**"]);
        assert!(path_matches(Path::new("ok/a.jsonl"), true, &inc, &exc));
        assert!(!path_matches(Path::new("skip/a.jsonl"), true, &inc, &exc));
        assert!(!path_matches(Path::new("skip/nested/x"), true, &inc, &exc));
    }

    #[test]
    fn exclude_overrides_include() {
        let inc = set(&["**/*.rs"]);
        let exc = set(&["target/**"]);
        assert!(path_matches(Path::new("src/main.rs"), false, &inc, &exc));
        assert!(!path_matches(
            Path::new("target/debug/x.rs"),
            false,
            &inc,
            &exc
        ));
    }

    #[test]
    fn include_specific_then_dir_prefix_exclude() {
        let inc = set(&["**/*.jsonl"]);
        let exc = set(&["-Users-ab-Dropbox-Documents-Obsidian-Vault/**"]);
        assert!(path_matches(
            Path::new("session/2026-04-27.jsonl"),
            false,
            &inc,
            &exc,
        ));
        assert!(!path_matches(
            Path::new("-Users-ab-Dropbox-Documents-Obsidian-Vault/foo.jsonl"),
            false,
            &inc,
            &exc,
        ));
    }
}
