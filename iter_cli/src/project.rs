//! Project slug derivation for `iter compose`.
//!
//! Slug source precedence matches `docker compose`:
//!
//! 1. `--project-name <name>` (CLI override) wins.
//! 2. `COMPOSE_PROJECT_NAME` env var, when the override is absent.
//! 3. Otherwise, the canonical basename of the compose file's parent
//!    directory.
//!
//! The normalisation function itself mirrors `compose-go`'s
//! [`NormalizeProjectName`][1]:
//!
//! 1. Lowercase (Unicode-aware, but only ASCII letters survive step 2).
//! 2. Drop any character outside `[a-z0-9_-]` — spaces, dots, and
//!    non-ASCII code points are silently stripped, not rejected. So
//!    `Obsidian Vault` becomes `obsidianvault`, `My.Project.v2` becomes
//!    `myprojectv2`.
//! 3. Trim leading `_` / `-`.
//!
//! **Where iter intentionally diverges from docker compose v2**:
//! docker compose only normalises the directory-basename fallback; for
//! `--project-name` and `COMPOSE_PROJECT_NAME` it instead errors when
//! the input is not already in normalised form. iter is more
//! permissive on those two paths — it runs the same normalisation
//! through them — so `-p "Foo Bar"` and `COMPOSE_PROJECT_NAME="Foo Bar"`
//! both succeed with slug `foobar` instead of failing. This is a
//! deliberate UX softening (the path `~/Dropbox/Documents/Obsidian
//! Vault` triggered it).
//!
//! Validation then enforces the docker-compose slug rule
//! (`[a-z0-9_-]+`, leading `[a-z0-9]`) on the normalised string. After
//! normalisation the only reachable failure is `Empty` (e.g. directory
//! named `!!!` strips to nothing); `BadLeading` / `BadCharacter` only
//! fire when [`validate`] is called directly with an unnormalised
//! string.
//!
//! [1]: https://github.com/compose-spec/compose-go/blob/main/loader/loader.go

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Environment variable consulted when no explicit override is passed.
pub(crate) const ENV_PROJECT_NAME: &str = "COMPOSE_PROJECT_NAME";

/// Errors returned by [`project_slug`].
#[derive(Debug, Error)]
pub(crate) enum ProjectSlugError {
    /// The compose file path has no parent directory we can use as a
    /// fallback (e.g. it was a bare filename with no on-disk presence).
    #[error("cannot derive project name: {compose_path:?} has no parent directory")]
    NoParent {
        /// Compose file path that triggered the failure.
        compose_path: PathBuf,
    },
    /// Canonicalising the parent directory failed.
    #[error("cannot canonicalise parent of {compose_path:?}: {source}")]
    Canonicalise {
        /// Compose file path whose parent we tried to canonicalise.
        compose_path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The parent directory's basename is empty (e.g. compose file at
    /// the filesystem root).
    #[error("cannot derive project name: parent of {compose_path:?} has no basename")]
    NoBasename {
        /// Compose file path that triggered the failure.
        compose_path: PathBuf,
    },
    /// The candidate slug failed the docker-compose validation rule.
    #[error(
        "{source} (consider passing -p / --project-name <name> or setting {env})",
        env = ENV_PROJECT_NAME
    )]
    Invalid {
        /// Underlying validation failure.
        #[source]
        source: SlugValidationError,
    },
}

/// Reasons a candidate string is rejected as a project slug.
///
/// Marked `#[non_exhaustive]` because new failure modes may be added
/// (e.g. length caps) without bumping the major version.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum SlugValidationError {
    /// Empty input.
    #[error("project name is empty")]
    Empty,
    /// First character is not `[a-z0-9]`.
    ///
    /// Unreachable through [`project_slug`] — `normalise()` strips
    /// leading `_` / `-` before validation. Only fires when
    /// [`validate`] is called directly with an unnormalised input.
    #[error("project name {name:?} must start with [a-z0-9] (got {first:?})")]
    BadLeading {
        /// The full rejected name.
        name: String,
        /// The offending leading character.
        first: char,
    },
    /// One or more characters are outside `[a-z0-9_-]`.
    ///
    /// Unreachable through [`project_slug`] — `normalise()` filters out
    /// every character outside `[a-z0-9_-]` before validation. Only
    /// fires when [`validate`] is called directly with an unnormalised
    /// input.
    #[error("project name {name:?} contains invalid character {bad:?} (allowed: a-z 0-9 _ -)")]
    BadCharacter {
        /// The full rejected name.
        name: String,
        /// The first invalid character encountered.
        bad: char,
    },
}

/// Derive the docker-compose-style project slug for a compose run.
///
/// `compose_path` should be the user-facing compose file path (after
/// the `-f` resolution). `override_name`, if present, takes precedence
/// over both the env var and the directory-basename fallback.
///
/// # Errors
///
/// Returns [`ProjectSlugError`] when no slug source is available or the
/// derived candidate fails docker-compose validation.
pub(crate) fn project_slug(
    compose_path: &Path,
    override_name: Option<&str>,
) -> Result<String, ProjectSlugError> {
    if let Some(name) = override_name {
        return validate(&normalise(name));
    }
    if let Ok(env) = std::env::var(ENV_PROJECT_NAME)
        && !env.is_empty()
    {
        return validate(&normalise(&env));
    }
    let parent = compose_path
        .parent()
        .ok_or_else(|| ProjectSlugError::NoParent {
            compose_path: compose_path.to_owned(),
        })?;
    // `Path::parent` of `"compose.iter"` is `Some("")` — treat that as
    // "current directory" the same way docker compose does.
    let resolved = if parent.as_os_str().is_empty() {
        std::env::current_dir().map_err(|source| ProjectSlugError::Canonicalise {
            compose_path: compose_path.to_owned(),
            source,
        })?
    } else {
        parent
            .canonicalize()
            .map_err(|source| ProjectSlugError::Canonicalise {
                compose_path: compose_path.to_owned(),
                source,
            })?
    };
    let basename = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ProjectSlugError::NoBasename {
            compose_path: compose_path.to_owned(),
        })?;
    let normalised = normalise(basename);
    validate(&normalised)
}

/// Lowercase, then strip everything outside `[a-z0-9_-]`, then trim
/// leading `_` / `-`. Equivalent to `compose-go`'s
/// `NormalizeProjectName`: spaces, dots, and non-ASCII code points are
/// silently dropped rather than rejected.
fn normalise(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect();
    kept.trim_start_matches(['_', '-']).to_owned()
}

/// Public for callers that want to validate a slug they already have
/// (e.g. CLI flag parsing).
///
/// # Errors
///
/// Returns [`ProjectSlugError::Invalid`] when the input violates the
/// docker-compose slug rule.
pub(crate) fn validate(name: &str) -> Result<String, ProjectSlugError> {
    let mut chars = name.chars();
    let first = chars.next().ok_or(ProjectSlugError::Invalid {
        source: SlugValidationError::Empty,
    })?;
    if !is_valid_leading(first) {
        return Err(ProjectSlugError::Invalid {
            source: SlugValidationError::BadLeading {
                name: name.to_owned(),
                first,
            },
        });
    }
    for c in chars {
        if !is_valid_body(c) {
            return Err(ProjectSlugError::Invalid {
                source: SlugValidationError::BadCharacter {
                    name: name.to_owned(),
                    bad: c,
                },
            });
        }
    }
    Ok(name.to_owned())
}

fn is_valid_leading(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn is_valid_body(c: char) -> bool {
    is_valid_leading(c) || c == '_' || c == '-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(path: &Path) {
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn override_wins_over_basename() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("dir-name");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        // Override beats the basename fallback. (Env-var precedence is
        // exercised manually; we deliberately do not mutate the
        // process-wide env here because tests share a single process and
        // env mutation would race other tests.)
        let slug = project_slug(&compose, Some("explicit-name")).expect("ok");
        assert_eq!(slug, "explicit-name");
    }

    #[test]
    fn falls_back_to_parent_basename() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("my-project");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, None).expect("ok");
        assert_eq!(slug, "my-project");
    }

    #[test]
    fn lowercases_uppercase_basename() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("CamelCase");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, None).expect("ok");
        assert_eq!(slug, "camelcase");
    }

    #[test]
    fn normalises_basename_with_spaces() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("Obsidian Vault");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, None).expect("ok");
        assert_eq!(slug, "obsidianvault");
    }

    #[test]
    fn normalises_basename_with_dots() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("My.Project.v2");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, None).expect("ok");
        assert_eq!(slug, "myprojectv2");
    }

    #[test]
    fn trims_leading_dash_and_underscore_from_basename() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("--__foo-bar");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, None).expect("ok");
        assert_eq!(slug, "foo-bar");
    }

    #[test]
    fn rejects_basename_that_strips_to_empty() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("!!!");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let err = project_slug(&compose, None).expect_err("empty after strip");
        assert!(matches!(
            err,
            ProjectSlugError::Invalid {
                source: SlugValidationError::Empty
            }
        ));
    }

    #[test]
    fn override_is_normalised_not_just_validated() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("ignored");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, Some("Foo Bar")).expect("ok");
        assert_eq!(slug, "foobar");
    }

    #[test]
    fn override_normalisation_preserves_existing_valid_chars() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("ignored");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, Some("a_b-c-1")).expect("ok");
        assert_eq!(slug, "a_b-c-1");
    }

    #[test]
    fn override_with_leading_dash_is_trimmed() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("ignored");
        std::fs::create_dir(&project_dir).unwrap();
        let compose = project_dir.join("compose.iter");
        touch(&compose);
        let slug = project_slug(&compose, Some("-leading-dash")).expect("ok");
        assert_eq!(slug, "leading-dash");
    }

    #[test]
    fn validate_allows_leading_digit_and_rejects_leading_dash() {
        // Leading digit is fine — docker compose allows it.
        let ok = validate("9-things").expect("digit leader allowed");
        assert_eq!(ok, "9-things");

        // `validate` itself is unchanged: passed a leading-dash string
        // directly, it still rejects. (Callers via `project_slug` go
        // through `normalise` first, so this branch is only reachable
        // by direct `validate` use.)
        let err = validate("-leading-dash").expect_err("leading dash bad");
        assert!(matches!(
            err,
            ProjectSlugError::Invalid {
                source: SlugValidationError::BadLeading { .. }
            }
        ));
    }

    #[test]
    fn rejects_empty_override() {
        let err = validate("").expect_err("empty");
        assert!(matches!(
            err,
            ProjectSlugError::Invalid {
                source: SlugValidationError::Empty
            }
        ));
    }

    #[test]
    fn allows_underscore_and_dash_in_body() {
        validate("a_b-c-1").expect("ok");
    }

    #[test]
    fn rejects_invalid_body_punctuation() {
        let err = validate("foo.bar").expect_err("dot bad");
        assert!(matches!(
            err,
            ProjectSlugError::Invalid {
                source: SlugValidationError::BadCharacter { .. }
            }
        ));
    }
}
