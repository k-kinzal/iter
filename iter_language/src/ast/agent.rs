//! `agent` declaration AST.

use std::collections::BTreeMap;

/// Agent backend declaration.
///
/// Every named variant (all but [`AgentDef::Generic`]) carries a concrete
/// `command` field and an `args` pass-through list. The semantic lowerer
/// fills `command` from the agent kind's conventional binary when the source
/// omits it, while explicit source values let authors pin a specific binary
/// path. The iter runtime still prepends its mode-specific default flags
/// (e.g. `--print`, `--oneshot`, `exec`) and appends `args` after them.
///
/// Every variant also carries an `env` map: key–value pairs that become
/// environment variables in the spawned agent child process. At runtime,
/// each declared key `NAME` can be overridden by setting `ITER_NAME` in
/// the runner process environment; undeclared `ITER_*` variables are
/// ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDef {
    /// Anthropic Claude Code agent.
    Claude {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults. Empty
        /// is allowed.
        args: Vec<String>,
        /// Optional file path (relative to the workspace cwd, unless
        /// absolute) where iter persists a stable Claude Code session id
        /// across iterations. `None` disables session persistence and
        /// each iteration runs as a fresh session.
        ///
        /// When set, the first invocation writes a fresh v4 UUID and
        /// every subsequent invocation reads the same file, making iter
        /// pass `--session-id <uuid>` so Claude Code resumes the same
        /// session. This is the on-ramp to the narrowest exploration mode:
        /// later iterations inherit prior agent context as well as workspace
        /// state.
        session_id_file: Option<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// `OpenAI` Codex agent.
    Codex {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Google Gemini agent.
    Gemini {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Nous Research Hermes Agent.
    Hermes {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Google Antigravity CLI agent (successor to Gemini CLI).
    Antigravity {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Optional conversation ID for session persistence across
        /// iterations. When set, `--conversation <id>` is passed to
        /// the `agy` binary so it resumes the same session.
        conversation_id: Option<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// GitHub Copilot agent.
    Copilot {
        /// Invocation mode for the underlying CLI. Required.
        mode: AgentMode,
        /// Binary name or absolute path.
        command: String,
        /// Override the subcommand inserted between the binary and the
        /// positional prompt. `None` leaves the agent's canonical
        /// subcommand in place (iter ships the agent-operational default);
        /// `Some(vec![])` explicitly invokes the binary with no
        /// subcommand; `Some(v)` replaces the subcommand entirely.
        subcommand: Option<Vec<String>>,
        /// Extra arguments appended between the subcommand and the
        /// positional prompt.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Cursor agent.
    Cursor {
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Cline agent.
    Cline {
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// `opencode` agent.
    OpenCode {
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed defaults.
        args: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// xAI Grok Build (`grok`) agent.
    ///
    /// Headless-first: iter drives the official `grok -p` headless mode.
    /// There is no `mode` field because the TUI path is out of scope for
    /// this driver.
    Grok {
        /// Binary name or absolute path.
        command: String,
        /// Extra arguments appended after the iter-managed headless flags.
        args: Vec<String>,
        /// Optional file path (relative to the workspace cwd, unless
        /// absolute) where iter persists a stable Grok session id across
        /// iterations. `None` disables session persistence and each
        /// iteration runs as a fresh session.
        ///
        /// When set, the first invocation writes a fresh v4 UUID and every
        /// subsequent invocation reads the same file, making iter pass
        /// `-s <uuid>` so Grok resumes the same headless session. This is
        /// the on-ramp to the narrowest exploration mode: later iterations
        /// inherit prior agent context as well as workspace state.
        session_id_file: Option<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Agent that does nothing. Exits immediately with success.
    Noop,
    /// Configurable fake agent for verification testing.
    Fake {
        /// Process exit code. 0 = success, non-zero = failure.
        exit_code: i32,
        /// Simulated execution delay in seconds. 0 = immediate.
        delay_secs: Option<u64>,
        /// Lines to write to stdout via the output sink.
        stdout: Vec<String>,
        /// Lines to write to stderr via the output sink.
        stderr: Vec<String>,
        /// Files to create/overwrite in the workspace directory.
        /// Keys are relative paths, values are file content.
        files: BTreeMap<String, String>,
    },
    /// Generic agent invoked through an arbitrary command vector.
    Generic {
        /// Argv-style command. The first element is the program; subsequent
        /// elements are arguments.
        command: Vec<String>,
        /// Environment variables passed to the agent child process.
        env: BTreeMap<String, String>,
    },
    /// Multi-agent router that dispatches to sub-agents based on strategy.
    Router {
        /// Named sub-agent declarations in priority/rotation order.
        agents: Vec<(String, Box<AgentDef>)>,
        /// How the router selects an agent each iteration.
        strategy: RouterStrategy,
        /// Failure classes that trigger fallback when `strategy = fallback`.
        fallback_on: RouterFallbackTriggers,
    },
}

/// Strategy for agent routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterStrategy {
    /// Use the first agent; fall back to the next on configured failures.
    Fallback,
    /// Rotate through agents round-robin across iterations.
    Rotate,
}

/// Router fallback trigger set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterFallbackTriggers {
    /// Fall back on every failure class except cancellation.
    Any,
    /// Fall back only on the listed failure classes.
    Only(std::collections::BTreeSet<RouterFallbackClass>),
}

/// Fallback-eligible agent failure classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouterFallbackClass {
    /// Iteration timeout.
    Timeout,
    /// Context-window or token-limit overflow.
    TokenLimit,
    /// Launch, auth, startup, or configuration failure.
    Launch,
    /// Process terminated by signal.
    TerminatedBySignal,
    /// Non-zero exit or in-band failure.
    Failure,
}

/// Agent invocation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// Interactive mode (TTY-attached).
    Interactive,
    /// Headless mode (no terminal; non-interactive, batch output). Spelled
    /// `print` in the grammar (the surface keyword is kept; the AST variant
    /// names the concept).
    Headless,
}
