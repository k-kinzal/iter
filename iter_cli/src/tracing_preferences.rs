//! Operator tracing preferences, read from `~/.iter/config.toml`.
//!
//! The only thing this file decides is `tracing` verbosity. The operator's
//! `~/.iter/config.toml` carries a single `log_level`, and the telemetry
//! init in [`crate::telemetry`] consumes it to pick the subscriber's level.
//!
//! Tracing verbosity is an operating concern, so the preferences live with
//! the operator surface (the CLI), beside the telemetry init they configure —
//! the core exploration never reads them. The on-disk format is unchanged:
//! the field is still serialized as `log_level`, so existing
//! `~/.iter/config.toml` files keep working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Verbosity for `tracing` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and above.
    Warn,
    /// Informational messages and above.
    Info,
    /// Debug messages and above.
    Debug,
    /// All messages including very verbose trace output.
    Trace,
}

impl LogLevel {
    /// Convert to a [`tracing::Level`] for use with `tracing-subscriber`.
    #[must_use]
    pub fn as_tracing_level(self) -> tracing::Level {
        match self {
            Self::Error => tracing::Level::ERROR,
            Self::Warn => tracing::Level::WARN,
            Self::Info => tracing::Level::INFO,
            Self::Debug => tracing::Level::DEBUG,
            Self::Trace => tracing::Level::TRACE,
        }
    }
}

/// Operator tracing preferences for the iter CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingPreferences {
    /// Logging verbosity.
    pub log_level: Option<LogLevel>,
}

/// Errors emitted by [`TracingPreferences::load`].
#[derive(Debug, thiserror::Error)]
pub enum TracingPreferencesError {
    /// I/O error reading the preferences file.
    #[error("failed to read preferences file: {0}")]
    Io(#[from] std::io::Error),

    /// The preferences file contained invalid TOML.
    #[error("failed to parse preferences file: {0}")]
    Parse(#[from] toml::de::Error),

    /// The user has no home directory and no explicit path was given.
    #[error("could not determine home directory for default preferences path")]
    NoHome,
}

impl TracingPreferences {
    /// Load tracing preferences from `path` (or [`TracingPreferences::default_path`]
    /// when `None`).
    ///
    /// When no explicit path is supplied, the default preferences file is
    /// **truly optional**: any I/O error while reading it (file missing,
    /// permission denied, parent directory unreadable, …) is silently
    /// downgraded to [`TracingPreferences::default`]. This keeps the CLI usable
    /// in sandboxes, containers, and CI environments that restrict access to
    /// the user's home directory.
    ///
    /// When a path is supplied explicitly, the caller has stated intent
    /// and errors are propagated verbatim — a missing or unreadable file
    /// is a hard failure.
    ///
    /// Parse errors are always propagated: the file was present and
    /// readable, the user wants to know it is malformed.
    ///
    /// # Errors
    ///
    /// Returns [`TracingPreferencesError::Io`] when an explicitly-supplied path
    /// cannot be read, [`TracingPreferencesError::Parse`] when the file is not
    /// valid TOML, and [`TracingPreferencesError::NoHome`] when the default
    /// path is requested but no home directory can be determined.
    pub fn load(path: Option<&Path>) -> Result<Self, TracingPreferencesError> {
        let (resolved, is_explicit): (PathBuf, bool) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => (
                Self::default_path().ok_or(TracingPreferencesError::NoHome)?,
                false,
            ),
        };

        match std::fs::read_to_string(&resolved) {
            Ok(text) => {
                let prefs: TracingPreferences = toml::from_str(&text)?;
                Ok(prefs)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) if !is_explicit => Ok(Self::default()),
            Err(err) => Err(TracingPreferencesError::Io(err)),
        }
    }

    /// Return the default path used when no override is supplied.
    ///
    /// Resolves to `~/.iter/config.toml` using the `HOME` environment
    /// variable on Unix or `USERPROFILE` on Windows. Returns `None` when no
    /// home directory can be determined.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)?;
        Some(home.join(".iter").join("config.toml"))
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("does-not-exist.toml");
        let prefs = TracingPreferences::load(Some(&path)).expect("load");
        assert_eq!(prefs, TracingPreferences::default());
    }

    #[test]
    fn parses_log_level() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "log_level = \"debug\"\n").expect("write");
        let prefs = TracingPreferences::load(Some(&path)).expect("load");
        assert_eq!(prefs.log_level, Some(LogLevel::Debug));
    }

    #[test]
    fn invalid_toml_returns_parse_error() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "log_level = \"not-a-real-level\"\n").expect("write");
        let err = TracingPreferences::load(Some(&path)).expect_err("expected parse error");
        assert!(matches!(err, TracingPreferencesError::Parse(_)));
    }

    #[test]
    fn truly_malformed_toml_returns_parse_error() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not toml = = = ").expect("write");
        let err = TracingPreferences::load(Some(&path)).expect_err("expected parse error");
        assert!(matches!(err, TracingPreferencesError::Parse(_)));
    }

    #[cfg(unix)]
    #[test]
    fn explicit_path_io_error_propagates() {
        // An unreadable explicit path must surface as `TracingPreferencesError::Io`
        // — the user asked for *this* file, we must not silently substitute
        // defaults.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "log_level = \"debug\"\n").expect("write");
        // Make the file unreadable to the current user.
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&path, perms).expect("chmod");

        let err = TracingPreferences::load(Some(&path)).expect_err("expected io error");
        assert!(matches!(err, TracingPreferencesError::Io(_)));

        // Restore so tempdir teardown can delete the file.
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn default_path_io_error_falls_back_to_default() {
        // When resolving the default path, permission errors (e.g., sandboxed
        // HOME) must be silently downgraded to the default preferences so the
        // CLI remains usable when the optional file is simply inaccessible.
        let dir = TempDir::new().expect("tempdir");
        // Point HOME at a path where `.iter/config.toml` exists but is
        // unreadable.
        let fake_home = dir.path().join("home");
        let iter_dir = fake_home.join(".iter");
        std::fs::create_dir_all(&iter_dir).expect("mkdir");
        let config_path = iter_dir.join("config.toml");
        std::fs::write(&config_path, "log_level = \"debug\"\n").expect("write");
        let mut perms = std::fs::metadata(&config_path).expect("meta").permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&config_path, perms).expect("chmod");

        let prev = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", &fake_home);
        }

        let result = TracingPreferences::load(None);

        // Restore HOME before asserting so a panic still leaves a sane env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        // Restore permissions so tempdir teardown can delete the file.
        let mut perms = std::fs::metadata(&config_path).expect("meta").permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&config_path, perms).expect("chmod");

        let prefs = result.expect("load should fall back on default-path io errors");
        assert_eq!(prefs, TracingPreferences::default());
    }

    #[test]
    fn default_path_resolves_when_home_set() {
        // SAFETY: tests are run in serial within a single tokio runtime; this
        // mutation only affects the local process and is reset below.
        let prev = std::env::var_os("HOME");
        // SAFETY: see above. set_var is `unsafe` in Rust 2024 edition.
        unsafe {
            std::env::set_var("HOME", "/tmp/iter-test-home");
        }
        let path = TracingPreferences::default_path().expect("default_path");
        assert!(path.ends_with(".iter/config.toml"));
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
