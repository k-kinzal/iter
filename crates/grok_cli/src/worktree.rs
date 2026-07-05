//! `grok worktree` — manage Grok-created git worktrees.
//!
//! `grok worktree <COMMAND>` groups `list`/`show`/`rm`/`gc` plus the nested
//! `db` maintenance group.

use std::ffi::OsString;

use crate::args::{ToArgs, push_flag, push_opt};
use crate::options::GlobalOptions;

/// A `grok worktree` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeSubcommand {
    /// `list` — list tracked worktrees.
    List {
        /// `--repo <REPO>`: filter to a repository.
        repo: Option<String>,
        /// `--type <TYPE>`: filter by worktree type.
        worktree_type: Option<String>,
        /// `--json`: emit machine-readable JSON output.
        json: bool,
        /// `--all`: include worktrees from every repository.
        all: bool,
    },
    /// `show <ID_OR_PATH>` — show details for a worktree.
    Show {
        /// Worktree id or path.
        id_or_path: String,
    },
    /// `rm <IDS>...` — remove one or more worktrees.
    ///
    /// The `<IDS>...` positional requires at least one value, so the ids are
    /// modeled as a required `first_id` plus any `rest_ids`; a zero-id `rm`
    /// (which grok rejects as a missing-argument usage error) is
    /// unrepresentable.
    Rm {
        /// First worktree id to remove (the required head of `<IDS>...`).
        first_id: String,
        /// Any further worktree ids to remove.
        rest_ids: Vec<String>,
        /// `-f, --force`: remove even with uncommitted changes.
        force: bool,
        /// `--dry-run`: show what would happen without making changes.
        dry_run: bool,
    },
    /// `gc` — garbage-collect stale worktrees.
    Gc {
        /// `--dry-run`: show what would happen without making changes.
        dry_run: bool,
        /// `--max-age <MAX_AGE>`: only collect worktrees older than this.
        max_age: Option<String>,
        /// `-f, --force`: remove even with uncommitted changes.
        force: bool,
    },
    /// `db <COMMAND>` — worktree database maintenance.
    Db(WorktreeDbSubcommand),
}

/// A `grok worktree db` subcommand.
///
/// The `db` leaves do not expose their own `--help` in `grok 0.2.82` (asking
/// for it prints the root help), so their flag sets are not verifiable from the
/// CLI. They are modeled structurally; append any extra flags through
/// [`WorktreeDbSubcommand::args`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeDbSubcommand {
    /// `rebuild` — rebuild the worktree database.
    Rebuild {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `stats` — show worktree database statistics.
    Stats {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
    /// `path` — print the worktree database path.
    Path {
        /// Extra args appended verbatim.
        args: Vec<String>,
    },
}

impl WorktreeDbSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("db".into());
        let (leaf, extra) = match self {
            Self::Rebuild { args } => ("rebuild", args),
            Self::Stats { args } => ("stats", args),
            Self::Path { args } => ("path", args),
        };
        args.push(leaf.into());
        for arg in extra {
            args.push(arg.into());
        }
    }
}

impl WorktreeSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::List {
                repo,
                worktree_type,
                json,
                all,
            } => {
                args.push("list".into());
                push_opt(args, "--repo", repo.as_deref());
                push_opt(args, "--type", worktree_type.as_deref());
                push_flag(args, *json, "--json");
                push_flag(args, *all, "--all");
            }
            Self::Show { id_or_path } => {
                args.push("show".into());
                args.push(id_or_path.into());
            }
            Self::Rm {
                first_id,
                rest_ids,
                force,
                dry_run,
            } => {
                args.push("rm".into());
                push_flag(args, *force, "--force");
                push_flag(args, *dry_run, "--dry-run");
                args.push(first_id.into());
                for id in rest_ids {
                    args.push(id.into());
                }
            }
            Self::Gc {
                dry_run,
                max_age,
                force,
            } => {
                args.push("gc".into());
                push_flag(args, *dry_run, "--dry-run");
                push_opt(args, "--max-age", max_age.as_deref());
                push_flag(args, *force, "--force");
            }
            Self::Db(db) => db.render(args),
        }
    }
}

/// `grok worktree [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCommand {
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub global: GlobalOptions,
    /// The `worktree` subcommand.
    pub command: WorktreeSubcommand,
}

impl WorktreeCommand {
    /// Build a `worktree` command for `subcommand`.
    #[must_use]
    pub fn new(subcommand: WorktreeSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command: subcommand,
        }
    }
}

impl ToArgs for WorktreeCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("worktree".into());
        self.global.render(args);
        self.command.render(args);
    }
}
