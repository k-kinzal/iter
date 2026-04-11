//! Construction-time configuration for [`ShellQueue`](super::ShellQueue).

use std::time::Duration;

/// Default per-enqueue timeout when the AST omits `enqueue_timeout`.
pub(super) const DEFAULT_ENQUEUE_TIMEOUT: Duration = Duration::from_secs(30);
/// Default interpreter when the AST omits `interpreter`.
pub(super) const DEFAULT_INTERPRETER: &str = "sh -c";

/// Construction-time configuration for [`ShellQueue`](super::ShellQueue).
///
/// Mirrors [`iter_language::QueueDecl::Shell`](iter_language::QueueDecl) one
/// for one. Convert via `ShellQueueConfig::from_decl_fields` inside the
/// CLI compose layer; the field-by-field constructor exists so unit tests
/// can build a config without round-tripping through the AST.
#[derive(Debug, Clone)]
pub struct ShellQueueConfig {
    /// Script run for every enqueue. Stdin receives the serialized envelope.
    pub enqueue: String,
    /// Long-lived script that emits NDJSON signal records on stdout.
    pub dequeue: String,
    /// Optional cleanup script run once at [`ShellQueue::close`](super::ShellQueue::close).
    pub close: Option<String>,
    /// Interpreter invocation. Defaults to `"sh -c"`.
    pub interpreter: Option<String>,
    /// Per-enqueue timeout. Defaults to 30s.
    pub enqueue_timeout: Option<Duration>,
}

impl ShellQueueConfig {
    /// Resolve [`Self::interpreter`] to the program + leading args list.
    pub(super) fn interpreter_argv(&self) -> Vec<String> {
        // Splitting on whitespace is sufficient for the documented
        // `<program> <flag>` shape (`sh -c`, `bash -c`, `zsh -c`). Anything
        // exotic is the user's problem to encode as a wrapper script.
        let raw = self.interpreter.as_deref().unwrap_or(DEFAULT_INTERPRETER);
        raw.split_whitespace().map(String::from).collect()
    }

    pub(super) fn enqueue_timeout(&self) -> Duration {
        self.enqueue_timeout.unwrap_or(DEFAULT_ENQUEUE_TIMEOUT)
    }
}
