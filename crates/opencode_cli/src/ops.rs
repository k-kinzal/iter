//! The remaining top-level operational commands.
//!
//! `completion`, `upgrade`, `uninstall`, `models`, `stats`, `export`,
//! `import`, `attach`, and `pr` each carry their own small flag or positional
//! surface on top of the shared [`GlobalOptions`].

use std::ffi::OsString;

use crate::args::{ToArgs, push_enum, push_flag, push_opt, push_opt_display};
use crate::options::GlobalOptions;
use crate::values::{Continuation, StatsModels, UpgradeMethod};

/// `opencode completion` — generate a shell completion script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
}

impl ToArgs for CompletionCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("completion".into());
        self.global.render(args);
    }
}

/// `opencode attach <url>` — attach to a running opencode server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `--dir <DIR>`: directory to run in.
    pub dir: Option<String>,
    /// Session continuation: `--continue` / `--session <id>` and the
    /// selector-gated `--fork`. Defaults to [`Continuation::Fresh`].
    pub continuation: Continuation,
    /// `-p, --password <PASSWORD>`: basic-auth password.
    pub password: Option<String>,
    /// The required `<url>` positional (e.g. `http://localhost:4096`).
    pub url: String,
}

impl ToArgs for AttachCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("attach".into());
        self.global.render(args);
        push_opt(args, "--dir", self.dir.as_deref());
        self.continuation.render(args);
        push_opt(args, "--password", self.password.as_deref());
        args.push((&self.url).into());
    }
}

/// `opencode upgrade [target]` — upgrade opencode to the latest or a specific
/// version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpgradeCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `-m, --method <METHOD>`: installation method to use.
    pub method: Option<UpgradeMethod>,
    /// Optional `[target]` version positional (e.g. `0.1.48`).
    pub target: Option<String>,
}

impl ToArgs for UpgradeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("upgrade".into());
        self.global.render(args);
        push_enum(args, "--method", self.method.map(UpgradeMethod::as_str));
        if let Some(target) = &self.target {
            args.push(target.into());
        }
    }
}

/// `opencode uninstall` — uninstall opencode and remove related files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `-c, --keep-config`: keep configuration files.
    pub keep_config: bool,
    /// `-d, --keep-data`: keep session data and snapshots.
    pub keep_data: bool,
    /// `--dry-run`: show what would be removed without removing.
    pub dry_run: bool,
    /// `-f, --force`: skip confirmation prompts.
    pub force: bool,
}

impl ToArgs for UninstallCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("uninstall".into());
        self.global.render(args);
        push_flag(args, self.keep_config, "--keep-config");
        push_flag(args, self.keep_data, "--keep-data");
        push_flag(args, self.dry_run, "--dry-run");
        push_flag(args, self.force, "--force");
    }
}

/// `opencode models [provider]` — list available models.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `--verbose`: include metadata like costs.
    pub verbose: bool,
    /// `--refresh`: refresh the models cache from models.dev.
    pub refresh: bool,
    /// Optional `[provider]` positional to filter by.
    pub provider: Option<String>,
}

impl ToArgs for ModelsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("models".into());
        self.global.render(args);
        push_flag(args, self.verbose, "--verbose");
        push_flag(args, self.refresh, "--refresh");
        if let Some(provider) = &self.provider {
            args.push(provider.into());
        }
    }
}

/// `opencode stats` — show token usage and cost statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// `--days <N>`: show stats for the last `N` days.
    pub days: Option<u32>,
    /// `--tools <N>`: number of tools to show.
    pub tools: Option<u32>,
    /// `--models`: model statistics display mode.
    pub models: Option<StatsModels>,
    /// `--project <PROJECT>`: filter by project (empty string = current).
    pub project: Option<String>,
}

impl ToArgs for StatsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("stats".into());
        self.global.render(args);
        push_opt_display(args, "--days", self.days);
        push_opt_display(args, "--tools", self.tools);
        match self.models {
            Some(StatsModels::All) => args.push("--models".into()),
            Some(StatsModels::Top(n)) => push_opt_display(args, "--models", Some(n)),
            None => {}
        }
        push_opt(args, "--project", self.project.as_deref());
    }
}

/// `opencode export [sessionID]` — export session data as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// Optional `[sessionID]` positional to export.
    pub session_id: Option<String>,
}

impl ToArgs for ExportCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("export".into());
        self.global.render(args);
        if let Some(session_id) = &self.session_id {
            args.push(session_id.into());
        }
    }
}

/// `opencode import <file>` — import session data from a JSON file or URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The required `<file>` positional: a path to a JSON file or a share URL.
    pub file: String,
}

impl ToArgs for ImportCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("import".into());
        self.global.render(args);
        args.push((&self.file).into());
    }
}

/// `opencode pr <number>` — fetch and check out a GitHub PR branch, then run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The required `<number>` positional: the PR number to check out.
    pub number: u64,
}

impl ToArgs for PrCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("pr".into());
        self.global.render(args);
        args.push(self.number.to_string().into());
    }
}
