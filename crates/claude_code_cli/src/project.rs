use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::push_flag;
use crate::values::Switch;

/// `claude project ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Project {
    /// `project purge`.
    Purge(ProjectPurge),
    /// `project help [command]`.
    Help(Option<ProjectHelpCommand>),
}

impl Project {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Purge(command) => {
                args.push("purge".into());
                command.render(args);
            }
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
        }
    }
}

/// `claude project help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectHelpCommand {
    /// `purge`.
    Purge,
}

impl ProjectHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Purge => "purge",
        }
    }
}

/// `claude project purge`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPurge {
    /// Purge target.
    pub target: Option<ProjectPurgeTarget>,
    /// `--dry-run`.
    pub dry_run: Switch,
    /// `--interactive`.
    pub interactive: Switch,
    /// `--yes`.
    pub yes: Switch,
}

impl ProjectPurge {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        if matches!(self.target, Some(ProjectPurgeTarget::All)) {
            args.push("--all".into());
        }
        push_flag(args, self.dry_run, "--dry-run");
        push_flag(args, self.interactive, "--interactive");
        push_flag(args, self.yes, "--yes");
        if let Some(ProjectPurgeTarget::Path(path)) = &self.target {
            args.push(path.into());
        }
    }
}

/// Mutually exclusive `claude project purge` target.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectPurgeTarget {
    /// Purge a specific project path.
    Path(PathBuf),
    /// Purge every project.
    All,
}
