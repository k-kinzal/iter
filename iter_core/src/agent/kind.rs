//! [`AgentKind`] — the object-safe discriminant of an [`Agent`](crate::Agent).
//!
//! Each concrete driver reports its variant through
//! [`Agent::kind`](crate::Agent::kind). The discriminant is a *closed,
//! fieldless* set: it carries no instance data, does **not** itself
//! `impl Agent`, and is never used for run-dispatch (that is the
//! `Box<dyn Agent>` trait object's job). Its single purpose is to let the
//! sandbox layer key per-agent OS-access policy off the agent without
//! downcasting to a concrete driver — see
//! [`SandboxProfile::for_agent`](crate::workspace::sandbox::SandboxProfile::for_agent),
//! which matches **exhaustively** over this enum so adding a kind without a
//! matching arm is a compile error (the no-omission guarantee).
//!
//! `AgentKind` deliberately mirrors the language-layer closed set
//! [`AgentDef`](iter_language::AgentDef): one variant per driver the
//! definition layer can name, plus the composite [`Router`](AgentKind::Router).

/// The kind of an [`Agent`](crate::Agent) — a closed, fieldless
/// discriminant used by the sandbox layer to select per-agent OS-access
/// policy without downcasting.
///
/// This is **not** an `Agent`: it neither runs nor dispatches. It is the
/// discriminant the sandbox layer matches on to give the run-time trait
/// object its per-kind profile.
///
/// The set is deliberately **closed** — not `#[non_exhaustive]` — mirroring
/// the language-layer [`AgentDef`](iter_language::AgentDef). A closed enum is
/// what makes the sandbox layer's exhaustive `match` a compile-time
/// no-omission check both inside `iter_core` and in any future consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    /// Anthropic Claude Code — [`ClaudeAgent`](crate::agent::ClaudeAgent).
    Claude,
    /// xAI Grok Build — [`GrokAgent`](crate::agent::GrokAgent).
    Grok,
    /// `OpenAI` Codex — [`CodexAgent`](crate::agent::CodexAgent).
    Codex,
    /// Google Gemini — [`GeminiAgent`](crate::agent::GeminiAgent).
    Gemini,
    /// Hermes — [`HermesAgent`](crate::agent::HermesAgent).
    Hermes,
    /// Antigravity — [`AntigravityAgent`](crate::agent::AntigravityAgent).
    Antigravity,
    /// GitHub Copilot CLI — [`CopilotAgent`](crate::agent::CopilotAgent).
    Copilot,
    /// Cursor Agent — [`CursorAgent`](crate::agent::CursorAgent).
    Cursor,
    /// Cline — [`ClineAgent`](crate::agent::ClineAgent).
    Cline,
    /// `OpenCode` — [`OpenCodeAgent`](crate::agent::OpenCodeAgent).
    OpenCode,
    /// Generic command-line agent — [`GenericAgent`](crate::agent::GenericAgent).
    Generic,
    /// In-process no-op agent — [`NoopAgent`](crate::agent::NoopAgent).
    Noop,
    /// In-process scripted fake agent — [`FakeAgent`](crate::agent::FakeAgent).
    Fake,
    /// Composite router — [`AgentRouter`](crate::agent::AgentRouter). The
    /// sandbox match's `Router` arm unions the profiles of the router's
    /// sub-agents (see [`Agent::sub_agents`](crate::Agent::sub_agents)).
    Router,
}
