//! `opencode agent` — manage agents.

use std::ffi::OsString;

use crate::args::{ToArgs, push_enum, push_opt};
use crate::options::GlobalOptions;
use crate::values::AgentMode;

/// `opencode agent <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The agent subcommand.
    pub command: AgentSubcommand,
}

impl AgentCommand {
    /// Wrap an agent subcommand with default global options.
    #[must_use]
    pub fn new(command: AgentSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for AgentCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("agent".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode agent` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentSubcommand {
    /// `agent create`: create a new agent. With no flags opencode prompts
    /// interactively; the flags below fill in the answers up front.
    Create {
        /// `--path <PATH>`: directory path to generate the agent file in.
        path: Option<String>,
        /// `--description <DESCRIPTION>`: what the agent should do.
        description: Option<String>,
        /// `--mode <all|primary|subagent>`: the agent mode.
        mode: Option<AgentMode>,
        /// `--tools <TOOLS>`: comma-separated list of tools to enable.
        tools: Option<String>,
        /// `-m, --model <PROVIDER/MODEL>`: the model to use.
        model: Option<String>,
    },
    /// `agent list`: list all available agents.
    List,
}

impl AgentSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Create {
                path,
                description,
                mode,
                tools,
                model,
            } => {
                args.push("create".into());
                push_opt(args, "--path", path.as_deref());
                push_opt(args, "--description", description.as_deref());
                push_enum(args, "--mode", mode.map(AgentMode::as_str));
                push_opt(args, "--tools", tools.as_deref());
                push_opt(args, "--model", model.as_deref());
            }
            Self::List => args.push("list".into()),
        }
    }
}
