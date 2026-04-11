//! ULID truncation for the human view.
//!
//! `iter ps` defaults to a 12-character prefix because a full 26-char
//! ULID dominates the line and the prefix is unique within any local
//! registry. `--no-trunc` recovers the full id.

/// Truncate `id` to 12 characters when `no_trunc == false`. When `id`
/// is shorter than 12 characters it is returned unchanged.
#[must_use]
pub(crate) fn trunc_id(id: &str, no_trunc: bool) -> String {
    const TRUNC_WIDTH: usize = 12;
    if no_trunc {
        return id.to_owned();
    }
    if id.len() <= TRUNC_WIDTH {
        return id.to_owned();
    }
    id.chars().take(TRUNC_WIDTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_to_twelve() {
        let ulid = "01h4zzzz0000000000abcdefgh";
        assert_eq!(ulid.len(), 26);
        let out = trunc_id(ulid, false);
        assert_eq!(out.len(), 12);
        assert!(ulid.starts_with(&out));
    }

    #[test]
    fn no_trunc_returns_full() {
        let ulid = "01h4zzzz0000000000abcdefgh";
        assert_eq!(trunc_id(ulid, true), ulid);
    }

    #[test]
    fn short_input_unchanged() {
        assert_eq!(trunc_id("abc", false), "abc");
        assert_eq!(trunc_id("abc", true), "abc");
    }

    #[test]
    fn handles_exactly_twelve() {
        let s = "abcdefghijkl";
        assert_eq!(trunc_id(s, false), s);
    }
}
