//! Configuration types for [`WatchTrigger`](super::WatchTrigger).

use std::path::PathBuf;
use std::time::Duration;

use globset::GlobSet;

use super::filter::compile_globset;

/// Choice of underlying filesystem watch backend.
///
/// Most callers should stick with [`WatchBackend::Recommended`]. The
/// [`WatchBackend::Poll`] variant exists for environments where the OS-native
/// backend isn't available (e.g., sandboxes that block `FSEvents` registration,
/// NFS mounts, or container runtimes without `inotify` inheritance).
#[derive(Debug, Clone, Default)]
pub enum WatchBackend {
    /// Let `notify` select the OS-native backend (`FSEvents` on macOS,
    /// `inotify` on Linux, `ReadDirectoryChangesW` on Windows).
    #[default]
    Recommended,
    /// Poll the filesystem at `interval`. Costlier and less precise than the
    /// native backends, but portable and works in sandboxes.
    #[allow(dead_code)]
    Poll {
        /// Polling interval. When `None`, defaults to 200 ms.
        interval: Option<Duration>,
    },
}

/// Configuration for [`WatchTrigger`](super::WatchTrigger).
///
/// Construct with [`WatchConfig::new`], which compiles the include / exclude
/// pattern lists into [`GlobSet`]s once. Patterns follow gitignore-style glob
/// semantics (`*` does not stop at `/`, `**` traverses directories), evaluated
/// against paths relative to [`Self::dir`].
///
/// # Backpressure
///
/// Internally the trigger uses an unbounded channel between the OS-side
/// watcher thread and the async processor. For typical workloads (config,
/// docs, session-log directories) this is fine. A storm of changes inside
/// the watched root — for example a `cargo build` writing thousands of
/// artefacts — can momentarily grow the channel until the processor drains
/// it. Place noisy build output behind an `exclude` glob, or watch a more
/// specific subdirectory, to keep the steady-state cheap.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Directory to watch (recursively).
    pub dir: PathBuf,
    /// Compiled include globs. Evaluated against paths relative to `dir`.
    /// Empty means "match everything" — see [`Self::include_empty`].
    pub include: GlobSet,
    /// Compiled exclude globs. A match here unconditionally rejects the path.
    pub exclude: GlobSet,
    /// `true` when the user supplied no include patterns. The matcher then
    /// accepts every path that is not in `exclude`.
    pub include_empty: bool,
    /// When `true`, emit one signal per file change. When `false`, coalesce
    /// events arriving inside the [`cooldown`](Self::cooldown) window into a
    /// single signal whose `files` metadata is a JSON array of paths.
    pub per_file: bool,
    /// Window used to coalesce / debounce events.
    ///
    /// - `per_file = false`: fixed-window batch. The deadline is set when
    ///   the first event of a batch arrives; further events are accumulated
    ///   until the deadline elapses, then one batch signal is emitted. The
    ///   deadline does *not* reset on subsequent events. When `None`, the
    ///   trigger uses an internal default of 250 ms.
    /// - `per_file = true`: per-path debounce. Repeated events on the same
    ///   path arriving inside `cooldown` are suppressed; the next event after
    ///   the window emits a fresh signal. `None` disables debouncing — every
    ///   event fires immediately.
    pub cooldown: Option<Duration>,
}

impl WatchConfig {
    /// Build a [`WatchConfig`], compiling the include / exclude patterns into
    /// [`GlobSet`]s. The first invalid glob short-circuits with the
    /// [`globset::Error`] from [`globset::Glob::new`].
    ///
    /// # Errors
    ///
    /// Returns the [`globset::Error`] from compiling the first invalid pattern
    /// in either `include_patterns` or `exclude_patterns`.
    pub fn new(
        dir: impl Into<PathBuf>,
        include_patterns: &[String],
        exclude_patterns: &[String],
        per_file: bool,
        cooldown: Option<Duration>,
    ) -> Result<Self, globset::Error> {
        let include = compile_globset(include_patterns)?;
        let exclude = compile_globset(exclude_patterns)?;
        Ok(Self {
            dir: dir.into(),
            include,
            exclude,
            include_empty: include_patterns.is_empty(),
            per_file,
            cooldown,
        })
    }
}
