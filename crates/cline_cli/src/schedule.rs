//! `cline schedule` — create and manage scheduled runs.
//!
//! Every leaf takes its own `--address <host:port>` (the hub server) and
//! `--json` switch; those render per-subcommand because Cline defines them on
//! the leaves, not on the `schedule` parent.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_enum, push_flag, push_opt, push_opt_num, push_opt_path};
use crate::values::AgentMode;

/// `cline schedule <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleCommand {
    /// The schedule subcommand.
    pub command: ScheduleSubcommand,
}

impl ScheduleCommand {
    /// Wrap a schedule subcommand.
    #[must_use]
    pub fn new(command: ScheduleSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for ScheduleCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("schedule".into());
        self.command.render(args);
    }
}

/// Options for `schedule create <name>` beyond the name positional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleCreateOptions {
    /// `--cron <pattern>`.
    pub cron: Option<String>,
    /// `--prompt <text>`: task prompt.
    pub prompt: Option<String>,
    /// `--workspace <path>`: workspace root path.
    pub workspace: Option<PathBuf>,
    /// `--created-by <name>`.
    pub created_by: Option<String>,
    /// `--cwd <path>`: working directory.
    pub cwd: Option<PathBuf>,
    /// `--disabled`: create in a disabled state.
    pub disabled: bool,
    /// `--max-parallel <n>` (Cline's default is `1`).
    pub max_parallel: Option<u32>,
    /// `--metadata-json <json>`: metadata as a JSON object.
    pub metadata_json: Option<String>,
    /// `--mode <act|plan>`: execution mode.
    pub mode: Option<AgentMode>,
    /// `--model <model>`.
    pub model: Option<String>,
    /// `--provider <id>` (Cline's default is `cline`).
    pub provider: Option<String>,
    /// `--system-prompt <text>`.
    pub system_prompt: Option<String>,
    /// `--tags <list>`: comma-separated tags.
    pub tags: Option<String>,
    /// `--timeout <seconds>`.
    pub timeout: Option<u64>,
    /// `--delivery-adapter <name>`.
    pub delivery_adapter: Option<String>,
    /// `--delivery-bot <name>`.
    pub delivery_bot: Option<String>,
    /// `--delivery-channel <id>`.
    pub delivery_channel: Option<String>,
    /// `--delivery-thread <id>`.
    pub delivery_thread: Option<String>,
    /// `--autonomous` / `--no-autonomous`: `Some(true)` enables autonomous
    /// mode, `Some(false)` disables it, `None` renders neither.
    pub autonomous: Option<bool>,
    /// `--idle-timeout <seconds>`: autonomous idle timeout.
    pub idle_timeout: Option<u64>,
    /// `--poll-interval <seconds>`: autonomous poll interval.
    pub poll_interval: Option<u64>,
    /// `--address <host:port>`: hub server address.
    pub address: Option<String>,
    /// `--json`: output as JSON.
    pub json: bool,
}

impl ScheduleCreateOptions {
    fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--cron", self.cron.as_deref());
        push_opt(args, "--prompt", self.prompt.as_deref());
        push_opt_path(args, "--workspace", self.workspace.as_deref());
        push_opt(args, "--created-by", self.created_by.as_deref());
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_flag(args, self.disabled, "--disabled");
        push_opt_num(args, "--max-parallel", self.max_parallel);
        push_opt(args, "--metadata-json", self.metadata_json.as_deref());
        push_enum(args, "--mode", self.mode.map(AgentMode::as_str));
        push_opt(args, "--model", self.model.as_deref());
        push_opt(args, "--provider", self.provider.as_deref());
        push_opt(args, "--system-prompt", self.system_prompt.as_deref());
        push_opt(args, "--tags", self.tags.as_deref());
        push_opt_num(args, "--timeout", self.timeout);
        push_opt(args, "--delivery-adapter", self.delivery_adapter.as_deref());
        push_opt(args, "--delivery-bot", self.delivery_bot.as_deref());
        push_opt(args, "--delivery-channel", self.delivery_channel.as_deref());
        push_opt(args, "--delivery-thread", self.delivery_thread.as_deref());
        match self.autonomous {
            Some(true) => args.push("--autonomous".into()),
            Some(false) => args.push("--no-autonomous".into()),
            None => {}
        }
        push_opt_num(args, "--idle-timeout", self.idle_timeout);
        push_opt_num(args, "--poll-interval", self.poll_interval);
        push_opt(args, "--address", self.address.as_deref());
        push_flag(args, self.json, "--json");
    }
}

/// Render the `--address`/`--json` pair every leaf shares.
fn push_address_json(args: &mut Vec<OsString>, address: Option<&str>, json: bool) {
    push_opt(args, "--address", address);
    push_flag(args, json, "--json");
}

/// A `cline schedule` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScheduleSubcommand {
    /// `schedule create <name>`: create a new schedule.
    ///
    /// The option set is large, so it is boxed to keep the enum compact.
    Create {
        /// Schedule name.
        name: String,
        /// Create options.
        options: Box<ScheduleCreateOptions>,
    },
    /// `schedule list`: list schedules.
    List {
        /// `--disabled`: show only disabled schedules.
        disabled: bool,
        /// `--enabled`: show only enabled schedules.
        enabled: bool,
        /// `--limit <n>` (Cline's default is `100`).
        limit: Option<u32>,
        /// `--tags <list>`: filter by comma-separated tags.
        tags: Option<String>,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule get <schedule-id>`.
    Get {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule delete <schedule-id>`.
    Delete {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule pause <schedule-id>`.
    Pause {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule resume <schedule-id>`.
    Resume {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule stats <schedule-id>`.
    Stats {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule trigger <schedule-id>`: trigger a schedule immediately.
    Trigger {
        /// Schedule ID.
        schedule_id: String,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule history <schedule-id>`: show execution history.
    History {
        /// Schedule ID.
        schedule_id: String,
        /// `--limit <n>` (Cline's default is `20`).
        limit: Option<u32>,
        /// `--status <status>`: filter by execution status.
        status: Option<String>,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule export <schedule-id>`.
    Export {
        /// Schedule ID.
        schedule_id: String,
        /// `--to <path>`: output file path.
        to: Option<PathBuf>,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule active`: show currently active executions.
    Active {
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
    /// `schedule upcoming`: show upcoming scheduled runs.
    Upcoming {
        /// `--limit <n>` (Cline's default is `20`).
        limit: Option<u32>,
        /// `--address <host:port>`.
        address: Option<String>,
        /// `--json`.
        json: bool,
    },
}

impl ScheduleSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Create { name, options } => {
                args.push("create".into());
                options.render(args);
                args.push(name.into());
            }
            Self::List {
                disabled,
                enabled,
                limit,
                tags,
                address,
                json,
            } => {
                args.push("list".into());
                push_flag(args, *disabled, "--disabled");
                push_flag(args, *enabled, "--enabled");
                push_opt_num(args, "--limit", *limit);
                push_opt(args, "--tags", tags.as_deref());
                push_address_json(args, address.as_deref(), *json);
            }
            Self::Get {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "get", schedule_id, address.as_deref(), *json),
            Self::Delete {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "delete", schedule_id, address.as_deref(), *json),
            Self::Pause {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "pause", schedule_id, address.as_deref(), *json),
            Self::Resume {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "resume", schedule_id, address.as_deref(), *json),
            Self::Stats {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "stats", schedule_id, address.as_deref(), *json),
            Self::Trigger {
                schedule_id,
                address,
                json,
            } => render_id_leaf(args, "trigger", schedule_id, address.as_deref(), *json),
            Self::History {
                schedule_id,
                limit,
                status,
                address,
                json,
            } => {
                args.push("history".into());
                push_opt_num(args, "--limit", *limit);
                push_opt(args, "--status", status.as_deref());
                args.push(schedule_id.into());
                push_address_json(args, address.as_deref(), *json);
            }
            Self::Export {
                schedule_id,
                to,
                address,
                json,
            } => {
                args.push("export".into());
                push_opt_path(args, "--to", to.as_deref());
                args.push(schedule_id.into());
                push_address_json(args, address.as_deref(), *json);
            }
            Self::Active { address, json } => {
                args.push("active".into());
                push_address_json(args, address.as_deref(), *json);
            }
            Self::Upcoming {
                limit,
                address,
                json,
            } => {
                args.push("upcoming".into());
                push_opt_num(args, "--limit", *limit);
                push_address_json(args, address.as_deref(), *json);
            }
        }
    }
}

/// Render a leaf whose only shape is `<name> <schedule-id> [--address] [--json]`.
fn render_id_leaf(
    args: &mut Vec<OsString>,
    name: &'static str,
    schedule_id: &str,
    address: Option<&str>,
    json: bool,
) {
    args.push(name.into());
    args.push(schedule_id.into());
    push_address_json(args, address, json);
}
