//! `cline plugin` — manage Cline plugins.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt_path};

/// `cline plugin <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// The plugin subcommand.
    pub command: PluginSubcommand,
}

impl PluginCommand {
    /// Wrap a plugin subcommand.
    #[must_use]
    pub fn new(command: PluginSubcommand) -> Self {
        Self { command }
    }
}

impl ToArgs for PluginCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("plugin".into());
        self.command.render(args);
    }
}

/// A `cline plugin` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginSubcommand {
    /// `plugin install <SOURCE>`: install from an official keyword, npm, git,
    /// URL, or local path.
    Install {
        /// Official keyword, npm package, git URL, plugin file URL, or local
        /// plugin path.
        source: String,
        /// `--npm`: treat the source as an npm package.
        npm: bool,
        /// `--git`: treat the source as a git repository.
        git: bool,
        /// `--force`: replace an existing install for the same source.
        force: bool,
        /// `--json`: output as JSON.
        json: bool,
        /// `--cwd <path>`: install to `<path>/.cline/plugins`.
        cwd: Option<PathBuf>,
    },
    /// `plugin uninstall <NAME>`: uninstall by name or path.
    Uninstall {
        /// Plugin package name, installed slug, or plugin path.
        name: String,
        /// `--json`: output as JSON.
        json: bool,
        /// `--cwd <path>`: search `<path>/.cline/plugins` before global
        /// plugins.
        cwd: Option<PathBuf>,
    },
}

impl PluginSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Install {
                source,
                npm,
                git,
                force,
                json,
                cwd,
            } => {
                args.push("install".into());
                push_flag(args, *npm, "--npm");
                push_flag(args, *git, "--git");
                push_flag(args, *force, "--force");
                push_flag(args, *json, "--json");
                push_opt_path(args, "--cwd", cwd.as_deref());
                args.push(source.into());
            }
            Self::Uninstall { name, json, cwd } => {
                args.push("uninstall".into());
                push_flag(args, *json, "--json");
                push_opt_path(args, "--cwd", cwd.as_deref());
                args.push(name.into());
            }
        }
    }
}
