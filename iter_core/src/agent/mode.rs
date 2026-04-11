//! [`AgentMode`] — selects how a hook-capable agent drives its underlying CLI.

use serde::{Deserialize, Serialize};

/// How a hook-capable agent drives its underlying CLI binary.
///
/// Four agents in this crate honour this enum —
/// [`ClaudeAgent`](crate::agent::ClaudeAgent),
/// [`CodexAgent`](crate::agent::CodexAgent),
/// [`GeminiAgent`](crate::agent::GeminiAgent), and
/// [`CopilotAgent`](crate::agent::CopilotAgent). The remaining four agents
/// ([`CursorAgent`](crate::agent::CursorAgent), [`ClineAgent`](crate::agent::ClineAgent),
/// [`OpenCodeAgent`](crate::agent::OpenCodeAgent), and
/// [`GenericAgent`](crate::agent::GenericAgent)) do not implement a hook-driven TUI
/// path and run in a single print-style shape regardless of the requested
/// mode.
///
/// * [`AgentMode::Interactive`] — launches the CLI as a live TUI session
///   and installs a project-local Stop-style hook under the CLI's own
///   config directory (`${cwd}/.claude/`, `${cwd}/.codex/`,
///   `${cwd}/.gemini/`, or `${cwd}/.github/hooks/`). The hook captures the
///   final assistant message and lets the CLI exit cleanly. This mirrors
///   `agent-loop`'s `claude-loop` / `codex-loop` / `gemini-loop` /
///   `copilot-loop` wrappers, except that iter's
///   [`Runner`](crate::Runner) handles per-signal iteration so the
///   hook only needs to capture state — no parent-process `kill -TERM`
///   loop driven by the hook script itself.
/// * [`AgentMode::Print`] — runs the CLI non-interactively in its
///   one-shot mode (`claude --print`, `codex exec`, `gemini -p`,
///   `gh copilot suggest`, etc.). The prompt is delivered inline or on
///   stdin and stdout is captured into
///   [`AgentReport::last_output`](crate::AgentReport). No tty
///   required; works in CI and detached instances.
///
/// There is no `Default` impl. Print vs Interactive is a project-shaped
/// decision: iter has no honest default because some workflows need the
/// TUI's hook bundle and others must run headless in CI. The Iterfile
/// must spell the choice out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentMode {
    /// Run the CLI under a live TUI with a project-local hook bundle.
    Interactive,
    /// Run the CLI in one-shot / print mode and capture stdout+stderr.
    Print,
}
