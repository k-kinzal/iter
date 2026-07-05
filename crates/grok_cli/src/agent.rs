//! `grok agent` — run Grok without the interactive UI.
//!
//! `grok agent [OPTIONS] [COMMAND]` hosts the non-UI agent transports:
//! `stdio` (ACP over stdio), `headless` (over the Grok WebSocket relay),
//! `serve` (as a WebSocket server), and `leader` (as the shared leader
//! process). iter does not drive this family — it uses the top-level headless
//! `-p` path (see [`single`](crate::single)) — but the surface is modeled for
//! completeness.
//!
//! The four transport leaves (`stdio`/`headless`/`serve`/`leader`) do not
//! expose their own `--help` in `grok 0.2.82` (asking for it prints the root
//! help), so their individual flag sets are not verifiable from the CLI. They
//! are modeled structurally as [`AgentTransport`] carrying the shared
//! [`GlobalOptions`]; add extra flags through [`AgentCommand::args`].

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt};
use crate::options::GlobalOptions;

/// The transport a `grok agent` run speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentTransport {
    /// `stdio` — run the agent over stdio (ACP).
    Stdio,
    /// `headless` — run headlessly over the Grok WebSocket relay.
    Headless,
    /// `serve` — run the agent as a WebSocket server.
    Serve,
    /// `leader` — run as the shared leader process for other clients.
    Leader,
}

impl AgentTransport {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Headless => "headless",
            Self::Serve => "serve",
            Self::Leader => "leader",
        }
    }
}

/// `grok agent [OPTIONS] [COMMAND]`.
///
/// The options here are the ones `grok agent --help` documents; the optional
/// [`transport`](Self::transport) selects a `stdio`/`headless`/`serve`/`leader`
/// leaf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The transport leaf, when one is selected.
    pub transport: Option<AgentTransport>,
    /// `--reauth`: run authentication before starting the agent.
    pub reauth: bool,
    /// `-m, --model <MODEL>`.
    pub model: Option<String>,
    /// `--reasoning-effort <EFFORT>`.
    pub reasoning_effort: Option<String>,
    /// `--always-approve`: auto-approve all tool executions.
    pub always_approve: bool,
    /// `--agent-profile <PATH>`: path to an agent profile file.
    pub agent_profile: Option<PathBuf>,
    /// `--leader`: connect to a shared leader process instead of starting a
    /// new agent.
    pub leader: bool,
    /// `--no-leader`: start a new agent even when config enables leader mode.
    pub no_leader: bool,
    /// `--grok-ws-origin <ORIGIN>`.
    pub grok_ws_origin: Option<String>,
    /// `--grok-ws-url <URL>`.
    pub grok_ws_url: Option<String>,
    /// `--cli-chat-proxy-base-url <URL>`: override the CLI chat proxy base URL.
    pub cli_chat_proxy_base_url: Option<String>,
    /// `--xai-api-base-url <URL>`: override the public xAI API base URL.
    pub xai_api_base_url: Option<String>,
    /// Extra args appended verbatim (e.g. transport-leaf flags this crate does
    /// not model).
    pub args: Vec<String>,
}

impl AgentCommand {
    /// Select the `stdio` transport leaf.
    #[must_use]
    pub fn stdio() -> Self {
        Self::with_transport(AgentTransport::Stdio)
    }

    /// Select the `headless` transport leaf.
    #[must_use]
    pub fn headless() -> Self {
        Self::with_transport(AgentTransport::Headless)
    }

    /// Select the `serve` transport leaf.
    #[must_use]
    pub fn serve() -> Self {
        Self::with_transport(AgentTransport::Serve)
    }

    /// Select the `leader` transport leaf.
    #[must_use]
    pub fn leader_transport() -> Self {
        Self::with_transport(AgentTransport::Leader)
    }

    fn with_transport(transport: AgentTransport) -> Self {
        Self {
            transport: Some(transport),
            ..Self::default()
        }
    }
}

impl ToArgs for AgentCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("agent".into());
        push_flag(args, self.reauth, "--reauth");
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--reasoning-effort", self.reasoning_effort.as_deref());
        push_flag(args, self.always_approve, "--always-approve");
        if let Some(profile) = &self.agent_profile {
            args.push("--agent-profile".into());
            args.push(profile.into());
        }
        push_flag(args, self.leader, "--leader");
        push_flag(args, self.no_leader, "--no-leader");
        push_opt(args, "--grok-ws-origin", self.grok_ws_origin.as_deref());
        push_opt(args, "--grok-ws-url", self.grok_ws_url.as_deref());
        push_opt(
            args,
            "--cli-chat-proxy-base-url",
            self.cli_chat_proxy_base_url.as_deref(),
        );
        push_opt(args, "--xai-api-base-url", self.xai_api_base_url.as_deref());
        self.global.render(args);
        if let Some(transport) = self.transport {
            args.push(transport.as_str().into());
        }
        for arg in &self.args {
            args.push(arg.into());
        }
    }
}
