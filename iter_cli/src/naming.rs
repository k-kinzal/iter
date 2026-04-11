//! Helpers for naming process records.
//!
//! The registry validates names tightly (no leading dot, ASCII only, no
//! control or path characters, ≤128 bytes — see
//! `iter_core::process::name_lock::name::validate_name`). When the user
//! does not pass `--name`, we synthesise a default from the iterfile
//! stem; this module is what makes that default safe to feed straight
//! into the registry.

use std::path::Path;

use iter_core::process::ProcessId;

/// Soft upper bound on a generated default name. Mirrors
/// `iter_core::process::name_lock::name::NAME_MAX_BYTES`; kept in lockstep
/// because the registry rejects longer values with `InvalidName`.
const MAX_NAME_BYTES: usize = 128;

/// Default human-friendly process name derived from the iterfile basename.
///
/// The basename is sanitised to satisfy `validate_name` (ASCII-only, no
/// leading dot, no path/control characters); the suffix is a ULID, which
/// is both collision-resistant and inside the allowed character set.
pub fn default_process_name(iterfile: &Path) -> String {
    let stem = iterfile
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "iter".to_owned());

    let suffix = ProcessId::generate().to_string();
    let stem = truncate_stem(&stem, suffix.len());
    format!("{stem}-{suffix}")
}

/// Replace anything outside `[A-Za-z0-9._-]` with `-`, then strip leading
/// dots so the result cannot trip the registry's "leading dot" rule.
fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_start_matches('.').to_owned()
}

/// Cap the stem so `<stem>-<suffix>` stays inside `MAX_NAME_BYTES`.
///
/// Trims on a UTF-8 char boundary to stay safe against non-ASCII input
/// even though `sanitize` already strips it; cheap and avoids panicking
/// if upstream policy ever loosens.
fn truncate_stem(stem: &str, suffix_len: usize) -> String {
    let budget = MAX_NAME_BYTES.saturating_sub(suffix_len + 1);
    if stem.len() <= budget {
        return stem.to_owned();
    }
    let mut end = budget;
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    stem[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ascii_stem_passes_through() {
        let name = default_process_name(&PathBuf::from("Iterfile"));
        assert!(name.starts_with("Iterfile-"));
        assert!(name.is_ascii());
    }

    #[test]
    fn dotfile_stem_drops_leading_dot() {
        let name = default_process_name(&PathBuf::from(".hidden"));
        // file_stem(".hidden") is ".hidden", which sanitize strips to ""
        // → fallback "iter".
        assert!(name.starts_with("iter-") || name.starts_with("hidden-"));
        assert!(!name.starts_with('.'));
    }

    #[test]
    fn non_ascii_stem_is_replaced_with_dashes() {
        let name = default_process_name(&PathBuf::from("日本語.iter"));
        assert!(name.is_ascii());
        // Non-ASCII chars → `-`, dotted suffix preserved.
        assert!(name.starts_with("---"));
    }

    #[test]
    fn overlong_stem_is_truncated() {
        let stem = "a".repeat(200);
        let path = PathBuf::from(format!("{stem}.iter"));
        let name = default_process_name(&path);
        assert!(name.len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn whitespace_and_forbidden_chars_become_dashes() {
        let name = default_process_name(&PathBuf::from("foo bar"));
        assert!(name.starts_with("foo-bar-"), "got {name}");
    }

    #[test]
    fn empty_or_unicode_stem_falls_back_to_iter() {
        let name = default_process_name(&PathBuf::from(".."));
        assert!(name.starts_with("iter-"));
    }
}
