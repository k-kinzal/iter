//! [`AgentKind`] — the object-safe discriminant of an
//! [`AgentDriver`](crate::agent::AgentDriver).
//!
//! Each concrete driver reports its variant through
//! [`AgentDriver::kind`](crate::agent::AgentDriver::kind). The discriminant
//! is a *closed, fieldless* set: it carries no instance data and is never
//! used for run-dispatch. Its purposes are:
//!
//! - letting the sandbox layer key per-driver OS-access policy off the
//!   driver without downcasting — see
//!   [`SandboxProfile::for_drivers`](crate::workspace::sandbox::SandboxProfile::for_drivers),
//!   which matches **exhaustively** over this enum so adding a kind without
//!   a matching arm is a compile error (the no-omission guarantee);
//! - providing the stable telemetry label ([`label`](AgentKind::label))
//!   recorded as `iter.agent.name` for the driver that actually ran.
//!
//! `AgentKind` deliberately mirrors the language-layer closed set
//! [`AgentDef`](iter_language::AgentDef)'s leaf variants: one variant per
//! driver the definition layer can name. Composition (`kind = router`) is
//! not a driver and therefore has no kind — it is the
//! [`Agent`](crate::agent::Agent)'s internal structure.

/// The kind of an [`AgentDriver`](crate::agent::AgentDriver) — a closed,
/// fieldless discriminant used by the sandbox layer to select per-driver
/// OS-access policy without downcasting, and as the source of the
/// per-driver telemetry label.
///
/// The set is deliberately **closed** — not `#[non_exhaustive]` — mirroring
/// the language-layer [`AgentDef`](iter_language::AgentDef). A closed enum is
/// what makes the sandbox layer's exhaustive `match` a compile-time
/// no-omission check both inside `iter_core` and in any future consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    /// Anthropic Claude Code — [`ClaudeCodeDriver`](crate::agent::ClaudeCodeDriver).
    Claude,
    /// xAI Grok Build — [`GrokDriver`](crate::agent::GrokDriver).
    Grok,
    /// `OpenAI` Codex — [`CodexDriver`](crate::agent::CodexDriver).
    Codex,
    /// Google Gemini — [`GeminiDriver`](crate::agent::GeminiDriver).
    Gemini,
    /// Hermes — [`HermesDriver`](crate::agent::HermesDriver).
    Hermes,
    /// Antigravity — [`AntigravityDriver`](crate::agent::AntigravityDriver).
    Antigravity,
    /// GitHub Copilot CLI — [`CopilotDriver`](crate::agent::CopilotDriver).
    Copilot,
    /// Cursor Agent — [`CursorDriver`](crate::agent::CursorDriver).
    Cursor,
    /// Cline — [`ClineDriver`](crate::agent::ClineDriver).
    Cline,
    /// `OpenCode` — [`OpenCodeDriver`](crate::agent::OpenCodeDriver).
    OpenCode,
    /// Generic command-line agent — [`GenericDriver`](crate::agent::GenericDriver).
    Generic,
    /// Shell no-op agent — [`NoopDriver`](crate::agent::NoopDriver).
    Noop,
    /// Shell-scripted fake agent — [`FakeDriver`](crate::agent::FakeDriver).
    Fake,
}

impl AgentKind {
    /// Stable, human-meaningful telemetry label for this driver kind.
    ///
    /// Recorded as the `iter.agent.name` span attribute for the driver that
    /// actually ran an attempt. A **label**, not a discriminant — the values
    /// are part of the operator-facing telemetry vocabulary and must stay
    /// stable across refactors.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Hermes => "hermes",
            Self::Antigravity => "antigravity",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::OpenCode => "opencode",
            Self::Generic => "generic",
            Self::Noop => "noop",
            Self::Fake => "fake",
        }
    }
}
