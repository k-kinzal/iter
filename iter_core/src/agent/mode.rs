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
/// * [`AgentMode::Headless`] — runs the CLI non-interactively in its
///   one-shot mode (`claude --print`, `codex exec`, `gemini -p`,
///   `gh copilot suggest`, etc.). The prompt is delivered inline or on
///   stdin and the CLI's machine-readable output is parsed by the per-CLI
///   Command into an [`AgentRun`](crate::AgentRun). No tty required; works
///   in CI and detached instances.
///
/// There is no `Default` impl. Headless vs Interactive is a project-shaped
/// decision: iter has no honest default because some workflows need the
/// TUI's hook bundle and others must run headless in CI. The declaration
/// must spell the choice out.
///
/// The grammar keyword for [`AgentMode::Headless`] stays `print` — it is a
/// user-facing surface kept stable across this rename (R16); only the Rust
/// variant names the concept (the no-terminal mode), not a particular CLI
/// flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentMode {
    /// Run the CLI under a live TUI with a project-local hook bundle.
    Interactive,
    /// Run the CLI with no terminal — its one-shot / headless mode — and
    /// capture stdout+stderr.
    Headless,
}
