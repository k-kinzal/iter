//! `gemini skills` — manage agent skills (alias `skill`).
//!
//! The leaf shapes are modeled from each leaf's own `gemini skills <leaf>
//! --help`: typed positionals and the documented per-leaf flags. `disable`,
//! `install`, `link`, and `uninstall` all take the fixed-choice `--scope`
//! (`user` / `workspace`); `install` and `link` additionally take `--consent`.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_enum, push_flag, push_opt_path};
use crate::values::Scope;

/// `gemini skills [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsCommand {
    /// `-d, --debug`.
    pub debug: bool,
    /// The skills subcommand.
    pub command: SkillsSubcommand,
}

impl SkillsCommand {
    /// Wrap a skills subcommand with default options.
    #[must_use]
    pub fn new(command: SkillsSubcommand) -> Self {
        Self {
            debug: false,
            command,
        }
    }
}

impl ToArgs for SkillsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("skills".into());
        push_flag(args, self.debug, "--debug");
        self.command.render(args);
    }
}

/// A `gemini skills` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillsSubcommand {
    /// `skills list [--all]`.
    List {
        /// `--all`.
        all: bool,
    },
    /// `skills enable <NAME>`.
    Enable {
        /// Skill name.
        name: String,
    },
    /// `skills disable <NAME> [--scope <SCOPE>]`.
    Disable {
        /// Skill name.
        name: String,
        /// `-s, --scope <SCOPE>`.
        scope: Option<Scope>,
    },
    /// `skills install <SOURCE> [--scope <SCOPE>] [--path <PATH>] [--consent]`.
    Install {
        /// Git repository URL or local path.
        source: String,
        /// `--scope <SCOPE>`.
        scope: Option<Scope>,
        /// `--path <PATH>`.
        path: Option<PathBuf>,
        /// `--consent`: acknowledge install risks and skip the prompt.
        consent: bool,
    },
    /// `skills link <PATH> [--scope <SCOPE>] [--consent]`.
    Link {
        /// Local path to link.
        path: PathBuf,
        /// `--scope <SCOPE>`.
        scope: Option<Scope>,
        /// `--consent`: acknowledge link risks and skip the prompt.
        consent: bool,
    },
    /// `skills uninstall <NAME> [--scope <SCOPE>]`.
    Uninstall {
        /// Skill name.
        name: String,
        /// `--scope <SCOPE>`: `user` (default) or `workspace`.
        scope: Option<Scope>,
    },
}

impl SkillsSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List { all } => {
                args.push("list".into());
                push_flag(args, *all, "--all");
            }
            Self::Enable { name } => {
                args.push("enable".into());
                args.push(name.into());
            }
            Self::Disable { name, scope } => {
                args.push("disable".into());
                args.push(name.into());
                push_enum(args, "--scope", scope.map(Scope::as_str));
            }
            Self::Install {
                source,
                scope,
                path,
                consent,
            } => {
                args.push("install".into());
                args.push(source.into());
                push_enum(args, "--scope", scope.map(Scope::as_str));
                push_opt_path(args, "--path", path.as_deref());
                push_flag(args, *consent, "--consent");
            }
            Self::Link {
                path,
                scope,
                consent,
            } => {
                args.push("link".into());
                args.push(path.into());
                push_enum(args, "--scope", scope.map(Scope::as_str));
                push_flag(args, *consent, "--consent");
            }
            Self::Uninstall { name, scope } => {
                args.push("uninstall".into());
                args.push(name.into());
                push_enum(args, "--scope", scope.map(Scope::as_str));
            }
        }
    }
}
