//! Name validation and `.tmp` filename round-trip.
//!
//! The name validation rules are tight on purpose: the resulting string is
//! used as a path component under `~/.iter/proc/.locks/`, so anything that
//! can climb out (`/`, `\`), reserve a system slot (`.`, `..`), or hide
//! from `ls` (`.<x>`) is rejected up front rather than at filesystem
//! depth. The `.tmp` round-trip is the inverse pair the janitor uses to
//! decide which orphan files it owns.

use crate::process::error::RegistryError;

/// Maximum length of a registered name (in bytes).
pub(super) const NAME_MAX_BYTES: usize = 128;

/// 32-character hex suffix length (16 bytes encoded).
pub(super) const SUFFIX_HEX_LEN: usize = 32;

/// Validate a candidate name. Called by `acquire` before touching the
/// filesystem so an obviously-bad name surfaces as `InvalidName` rather
/// than as a downstream `openat` error.
pub(super) fn validate_name(name: &str) -> Result<(), RegistryError> {
    if name.is_empty() {
        return Err(RegistryError::InvalidName {
            reason: "empty".into(),
        });
    }
    if name.len() > NAME_MAX_BYTES {
        return Err(RegistryError::InvalidName {
            reason: format!("longer than {NAME_MAX_BYTES} bytes"),
        });
    }
    if name == "." || name == ".." {
        return Err(RegistryError::InvalidName {
            reason: "reserved component".into(),
        });
    }
    if name.starts_with('.') {
        return Err(RegistryError::InvalidName {
            reason: "leading dot".into(),
        });
    }
    for c in name.chars() {
        if c == '/' || c == '\\' || c == '\0' {
            return Err(RegistryError::InvalidName {
                reason: format!("forbidden character: {c:?}"),
            });
        }
        if !c.is_ascii() {
            return Err(RegistryError::InvalidName {
                reason: "non-ASCII character".into(),
            });
        }
        if (c as u32) < 0x20 || c == '\x7f' {
            return Err(RegistryError::InvalidName {
                reason: "control character".into(),
            });
        }
    }
    Ok(())
}

/// `format!(".{name}.{suffix}.tmp")`. Pairs with [`parse_tmp_name`].
pub(super) fn tmp_name_format(name: &str, suffix_hex: &str) -> String {
    format!(".{name}.{suffix_hex}.tmp")
}

/// Inverse of [`tmp_name_format`]. Returns `Some(name)` when `filename`
/// matches `.<name>.<32hex>.tmp` and the embedded `name` survives
/// [`validate_name`]. The strict shape match is what protects the janitor
/// from deleting unrelated files that happen to live under `.locks/`.
pub(super) fn parse_tmp_name(filename: &str) -> Option<&str> {
    let stem = filename.strip_suffix(".tmp")?;
    let dot = stem.rfind('.')?;
    let suffix = &stem[dot + 1..];
    if suffix.len() != SUFFIX_HEX_LEN {
        return None;
    }
    if !suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return None;
    }
    let prefix = &stem[..dot];
    let name = prefix.strip_prefix('.')?;
    validate_name(name).ok()?;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_typical_names() {
        validate_name("foo").unwrap();
        validate_name("foo-bar_42").unwrap();
        validate_name("a.b.c").unwrap();
    }

    #[test]
    fn validate_name_rejects_paths_and_dots() {
        assert!(validate_name("").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("a\0b").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name("a\u{1F600}").is_err());
        let too_long = "a".repeat(NAME_MAX_BYTES + 1);
        assert!(validate_name(&too_long).is_err());
    }

    #[test]
    fn parse_tmp_name_round_trip() {
        let n = "foo";
        let s = "0123456789abcdef0123456789abcdef";
        let tmp = tmp_name_format(n, s);
        assert_eq!(parse_tmp_name(&tmp), Some(n));

        assert_eq!(parse_tmp_name("foo.tmp"), None);
        assert_eq!(parse_tmp_name(".foo.tmp"), None);
        assert_eq!(parse_tmp_name(".foo.short.tmp"), None);
        assert_eq!(
            parse_tmp_name(".foo.GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG.tmp"),
            None
        );
        let hidden = format!(".{}.{}.tmp", ".sneaky", s);
        assert_eq!(parse_tmp_name(&hidden), None);
    }

    #[test]
    fn parse_tmp_name_strict_shape() {
        let s32 = "0123456789abcdef0123456789abcdef";
        assert_eq!(parse_tmp_name(&format!(".alpha.{s32}.tmp")), Some("alpha"));
        assert!(parse_tmp_name(&format!(".alpha.{s32}1.tmp")).is_none());
        assert!(parse_tmp_name(&format!(".alpha.{}.tmp", &s32[..30])).is_none());
        assert!(parse_tmp_name(&format!(".alpha.{}.tmp", "Z".repeat(32))).is_none());
        let with_dots = format!(".my.svc.v2.{s32}.tmp");
        assert_eq!(parse_tmp_name(&with_dots), Some("my.svc.v2"));
    }
}
