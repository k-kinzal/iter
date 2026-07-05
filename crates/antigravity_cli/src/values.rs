//! Typed value types for Antigravity's value-taking flags.

use std::time::Duration;

/// Source importer for `agy plugin import [source]`.
///
/// `agy` imports plugins from one of two fixed importers; the CLI's own help
/// quotes the choice set "import plugins from `gemini` or `claude`". Modeling
/// it as an enum makes any other source unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportSource {
    /// `gemini` — import Gemini CLI plugins.
    Gemini,
    /// `claude` — import Claude Code plugins.
    Claude,
}

impl ImportSource {
    /// The positional token `agy plugin import` expects for this source.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Claude => "claude",
        }
    }
}

/// A Go-style duration for `--print-timeout`.
///
/// `agy` parses the flag with Go's `time.ParseDuration`, so the value must
/// carry a unit (`"5m0s"`, `"600s"`, `"1h30m"`). [`GoDuration::render`] emits a
/// string that `time.ParseDuration` accepts; it targets parse compatibility,
/// not byte-identical reproduction of Go's `time.Duration.String()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoDuration(Duration);

impl GoDuration {
    /// Wrap a [`std::time::Duration`].
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    /// Build a duration from a whole number of seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    /// Build a duration from a whole number of milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(Duration::from_millis(millis))
    }

    /// The wrapped [`std::time::Duration`].
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        self.0
    }

    /// Render the duration as a Go-`ParseDuration`-compatible string.
    pub(crate) fn render(self) -> String {
        if self.0.is_zero() {
            return "0s".to_owned();
        }

        let secs = self.0.as_secs();
        let sub_nanos = self.0.subsec_nanos();

        if secs == 0 {
            // Sub-second: pick the largest whole unit that represents it.
            if sub_nanos % 1_000_000 == 0 {
                return format!("{}ms", sub_nanos / 1_000_000);
            }
            if sub_nanos % 1_000 == 0 {
                return format!("{}us", sub_nanos / 1_000);
            }
            return format!("{sub_nanos}ns");
        }

        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;

        let seconds = if sub_nanos == 0 {
            format!("{seconds}s")
        } else {
            let frac = format!("{sub_nanos:09}");
            let frac = frac.trim_end_matches('0');
            format!("{seconds}.{frac}s")
        };

        if hours > 0 {
            format!("{hours}h{minutes}m{seconds}")
        } else if minutes > 0 {
            format!("{minutes}m{seconds}")
        } else {
            seconds
        }
    }
}
