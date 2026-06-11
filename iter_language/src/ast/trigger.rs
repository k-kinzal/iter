//! `trigger` declaration AST and supporting trigger-payload types.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{PriorityKeyword, Value};

/// Trigger declaration. Triggers generate signals for the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerDef {
    /// `loop` trigger — fires repeatedly with optional bounds.
    Loop {
        /// Maximum number of iterations to perform, if any.
        max_iteration: Option<i64>,
        /// Delay between iterations, normalised to seconds.
        delay_secs: Option<i64>,
    },
    /// `cron` trigger — fires according to a cron schedule.
    Cron {
        /// Cron expression. Required.
        schedule: String,
        /// Optional IANA time zone name. `None` means UTC.
        timezone: Option<String>,
        /// Emit a single `startup = true` signal before entering the schedule.
        at_startup: bool,
        /// Window (in seconds) for catching up a missed tick on startup.
        /// `None` disables catch-up.
        catch_up_secs: Option<i64>,
        /// Maximum jitter (in seconds) added to each tick. `None` disables.
        jitter_secs: Option<i64>,
        /// Trigger-level base metadata copied into every emitted signal.
        base_metadata: Vec<(String, String)>,
        /// Priority assigned to every emitted signal.
        priority: Option<PriorityKeyword>,
        /// Stop after emitting this many signals. `None` means no limit.
        max_signals: Option<u64>,
    },
    /// `watch` trigger — fires on filesystem changes inside `dir`.
    Watch {
        /// Directory to monitor. Required.
        dir: String,
        /// Glob patterns of files to include. Empty means "match everything".
        include: Vec<String>,
        /// Glob patterns of files to exclude. Always wins over `include`.
        exclude: Vec<String>,
        /// Event kinds to emit. Empty means all kinds (`created`, `modified`,
        /// `removed`).
        kinds: Vec<WatchEventKind>,
        /// Whether to fire one signal per file or batch them.
        per_file: bool,
        /// Publish interval in seconds. Events arriving within this window
        /// are merged into a single signal. Does not suppress per-path
        /// events — all observed changes are preserved in signal metadata.
        interval_secs: Option<i64>,
        /// Trigger-level base metadata copied into every emitted signal.
        base_metadata: Vec<(String, String)>,
        /// Priority assigned to every emitted signal.
        priority: Option<PriorityKeyword>,
        /// Stop after emitting this many signals. `None` means no limit.
        max_signals: Option<u64>,
    },
    /// `files` trigger — drains one or more file-path sources in order.
    Files {
        /// Ordered list of sources to drain.
        sources: Vec<FilesSource>,
        /// If `true`, park on cancellation after draining every source
        /// instead of exiting.
        no_exit_on_eof: bool,
        /// Trigger-level base metadata copied into every emitted signal.
        base_metadata: Vec<(String, String)>,
        /// Priority assigned to every emitted signal.
        priority: Option<PriorityKeyword>,
        /// Stop after emitting this many signals. `None` means no limit.
        max_signals: Option<u64>,
    },
    /// `command` trigger — runs an external command and fans out its output.
    Command {
        /// The command to execute. Required.
        run: String,
        /// Shell prefix used to interpret `run`. Defaults to `sh -c` when
        /// lowered.
        shell: Option<String>,
        /// Optional extraction expression applied to the command output.
        extract: Option<ExtractExpr>,
        /// Polling interval in seconds.
        poll_secs: Option<i64>,
        /// Skip records observed in earlier polls.
        dedupe: bool,
        /// Behaviour when the polled command exits non-zero.
        on_error: Option<OnErrorKeyword>,
        /// Trigger-level base metadata copied into every emitted signal.
        base_metadata: Vec<(String, String)>,
        /// Priority assigned to every emitted signal.
        priority: Option<PriorityKeyword>,
        /// Stop after emitting this many signals. `None` means no limit.
        max_signals: Option<u64>,
    },
    /// `webhook` trigger — exposes an HTTP listener with per-event routes.
    Webhook {
        /// Host the listener should bind to. Defaults to `0.0.0.0` when
        /// lowered and `bind` is absent.
        host: Option<String>,
        /// Port the listener should bind to. Required when `bind` is absent.
        port: Option<i64>,
        /// Full `ADDR:PORT` string. Mutually exclusive with `host`+`port`.
        bind: Option<String>,
        /// HTTP path the listener should serve. Required.
        path: String,
        /// Optional shared secret used to verify incoming payloads.
        secret: Option<SecretExpr>,
        /// Per-event route declarations.
        routes: Vec<Subscription>,
        /// Trigger-level base metadata inherited by every route that does not
        /// set its own keys.
        base_metadata: Vec<(String, String)>,
        /// Trigger-level default priority used when a route does not set one.
        priority: Option<PriorityKeyword>,
        /// Stop after emitting this many signals. `None` means no limit.
        max_signals: Option<u64>,
    },
    /// External, user-defined trigger. The contents are preserved verbatim
    /// as a generic [`Value`] tree.
    External {
        /// Trigger kind name as written in source.
        name: String,
        /// Field bag preserved verbatim.
        config: BTreeMap<String, Value>,
    },
}

/// Source of file paths consumed by a `files` trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesSource {
    /// Read paths from standard input, one per line.
    Stdin,
    /// Read paths from the file at this path.
    Path(String),
}

/// Extraction expression for a `command` trigger.
///
/// Currently only `regex(...)` is exposed. The enum shape is preserved
/// so future built-in extractors (anything iter can ship without forcing
/// an external binary into PATH) can join without breaking match arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractExpr {
    /// `regex(<pattern>)` capture.
    Regex(String),
}

/// Webhook secret expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretExpr {
    /// String literal value.
    Literal(String),
    /// `env("VAR")` reference resolved at runtime.
    EnvVar(String),
    /// `file("./path")` reference resolved at runtime.
    File(PathBuf),
}

/// Event kind accepted by `kinds = [...]` on watch triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchEventKind {
    /// File creation events.
    Created,
    /// File modification events.
    Modified,
    /// File removal events.
    Removed,
}

/// Behaviour keyword accepted by `on_error = ...` on command triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnErrorKeyword {
    /// Log the failure and try again on the next poll (default).
    Continue,
    /// Stop the binary on the first command error.
    Abort,
    /// Silently swallow errors and continue.
    Skip,
}

/// One per-event route inside a `trigger webhook` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// Quoted event name pattern, e.g. `"issues.opened"`.
    pub event_pattern: String,
    /// Optional `when "<expr>"` guard. Stored as a raw string; its
    /// `{{event.*}}` references are checked at analysis time and the guard
    /// is evaluated by the webhook source when a matching event arrives.
    pub when: Option<String>,
    /// Priority keyword to assign to signals produced by this route.
    pub priority: Option<PriorityKeyword>,
    /// Metadata template fields, in source order. Each value is a raw
    /// template string with `{{...}}` placeholders left intact.
    pub metadata: Vec<(String, String)>,
}
