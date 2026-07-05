//! `opencode debug` — debugging and troubleshooting tools.
//!
//! opencode 1.2.20 exposes per-leaf `--help` for every `debug` subcommand,
//! including the nested trees (`lsp`, `rg`, `file`, `snapshot`). Each nested
//! tree is modeled by its own subcommand enum so the leaves and their flags
//! render as typed argv rather than an opaque passthrough.

use std::ffi::OsString;

use crate::args::{ToArgs, push_each, push_opt, push_opt_display};
use crate::options::GlobalOptions;

/// `opencode debug <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugCommand {
    /// Shared `--print-logs` / `--log-level` options.
    pub global: GlobalOptions,
    /// The debug subcommand.
    pub command: DebugSubcommand,
}

impl DebugCommand {
    /// Wrap a debug subcommand with default global options.
    #[must_use]
    pub fn new(command: DebugSubcommand) -> Self {
        Self {
            global: GlobalOptions::default(),
            command,
        }
    }
}

impl ToArgs for DebugCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("debug".into());
        self.global.render(args);
        self.command.render(args);
    }
}

/// An `opencode debug` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugSubcommand {
    /// `debug config`: show the resolved configuration.
    Config,
    /// `debug lsp <COMMAND>`: LSP debugging utilities.
    Lsp(DebugLspSubcommand),
    /// `debug rg <COMMAND>`: ripgrep debugging utilities.
    Rg(DebugRgSubcommand),
    /// `debug file <COMMAND>`: file-system debugging utilities.
    File(DebugFileSubcommand),
    /// `debug scrap`: list all known projects.
    Scrap,
    /// `debug skill`: list all available skills.
    Skill,
    /// `debug snapshot <COMMAND>`: snapshot debugging utilities.
    Snapshot(DebugSnapshotSubcommand),
    /// `debug agent <name>`: show agent configuration details.
    Agent {
        /// Agent name.
        name: String,
        /// `--tool <TOOL>`: tool id to execute.
        tool: Option<String>,
        /// `--params <PARAMS>`: tool params as JSON or a JS object literal.
        params: Option<String>,
    },
    /// `debug paths`: show global paths (data, config, cache, state).
    Paths,
    /// `debug wait`: wait indefinitely (for debugging).
    Wait,
}

impl DebugSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Config => args.push("config".into()),
            Self::Lsp(command) => command.render(args),
            Self::Rg(command) => command.render(args),
            Self::File(command) => command.render(args),
            Self::Scrap => args.push("scrap".into()),
            Self::Skill => args.push("skill".into()),
            Self::Snapshot(command) => command.render(args),
            Self::Agent {
                name,
                tool,
                params,
            } => {
                args.push("agent".into());
                push_opt(args, "--tool", tool.as_deref());
                push_opt(args, "--params", params.as_deref());
                args.push(name.into());
            }
            Self::Paths => args.push("paths".into()),
            Self::Wait => args.push("wait".into()),
        }
    }
}

/// An `opencode debug lsp` subcommand — LSP debugging utilities.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugLspSubcommand {
    /// `debug lsp diagnostics <file>`: get diagnostics for a file.
    Diagnostics {
        /// The file to inspect.
        file: String,
    },
    /// `debug lsp symbols <query>`: search workspace symbols.
    Symbols {
        /// The symbol query.
        query: String,
    },
    /// `debug lsp document-symbols <uri>`: get symbols from a document.
    DocumentSymbols {
        /// The document URI.
        uri: String,
    },
}

impl DebugLspSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("lsp".into());
        match self {
            Self::Diagnostics { file } => {
                args.push("diagnostics".into());
                args.push(file.into());
            }
            Self::Symbols { query } => {
                args.push("symbols".into());
                args.push(query.into());
            }
            Self::DocumentSymbols { uri } => {
                args.push("document-symbols".into());
                args.push(uri.into());
            }
        }
    }
}

/// An `opencode debug rg` subcommand — ripgrep debugging utilities.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugRgSubcommand {
    /// `debug rg tree`: show the file tree using ripgrep.
    Tree {
        /// `--limit <N>`: limit the number of results.
        limit: Option<u32>,
    },
    /// `debug rg files`: list files using ripgrep.
    Files {
        /// `--query <QUERY>`: filter files by query.
        query: Option<String>,
        /// `--glob <GLOB>`: glob pattern to match files.
        glob: Option<String>,
        /// `--limit <N>`: limit the number of results.
        limit: Option<u32>,
    },
    /// `debug rg search <pattern>`: search file contents using ripgrep.
    Search {
        /// The search pattern.
        pattern: String,
        /// `--glob <GLOB>` (repeatable): file glob patterns.
        glob: Vec<String>,
        /// `--limit <N>`: limit the number of results.
        limit: Option<u32>,
    },
}

impl DebugRgSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("rg".into());
        match self {
            Self::Tree { limit } => {
                args.push("tree".into());
                push_opt_display(args, "--limit", *limit);
            }
            Self::Files {
                query,
                glob,
                limit,
            } => {
                args.push("files".into());
                push_opt(args, "--query", query.as_deref());
                push_opt(args, "--glob", glob.as_deref());
                push_opt_display(args, "--limit", *limit);
            }
            Self::Search {
                pattern,
                glob,
                limit,
            } => {
                args.push("search".into());
                push_each(args, "--glob", glob);
                push_opt_display(args, "--limit", *limit);
                args.push(pattern.into());
            }
        }
    }
}

/// An `opencode debug file` subcommand — file-system debugging utilities.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugFileSubcommand {
    /// `debug file read <path>`: read file contents as JSON.
    Read {
        /// The file path to read.
        path: String,
    },
    /// `debug file status`: show file status information.
    Status,
    /// `debug file list <path>`: list files in a directory.
    List {
        /// The directory path to list.
        path: String,
    },
    /// `debug file search <query>`: search files by query.
    Search {
        /// The search query.
        query: String,
    },
    /// `debug file tree [dir]`: show the directory tree.
    Tree {
        /// Optional directory to tree (opencode defaults to the cwd).
        dir: Option<String>,
    },
}

impl DebugFileSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("file".into());
        match self {
            Self::Read { path } => {
                args.push("read".into());
                args.push(path.into());
            }
            Self::Status => args.push("status".into()),
            Self::List { path } => {
                args.push("list".into());
                args.push(path.into());
            }
            Self::Search { query } => {
                args.push("search".into());
                args.push(query.into());
            }
            Self::Tree { dir } => {
                args.push("tree".into());
                if let Some(dir) = dir {
                    args.push(dir.into());
                }
            }
        }
    }
}

/// An `opencode debug snapshot` subcommand — snapshot debugging utilities.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugSnapshotSubcommand {
    /// `debug snapshot track`: track the current snapshot state.
    Track,
    /// `debug snapshot patch <hash>`: show the patch for a snapshot hash.
    Patch {
        /// The snapshot hash.
        hash: String,
    },
    /// `debug snapshot diff <hash>`: show the diff for a snapshot hash.
    Diff {
        /// The snapshot hash.
        hash: String,
    },
}

impl DebugSnapshotSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        args.push("snapshot".into());
        match self {
            Self::Track => args.push("track".into()),
            Self::Patch { hash } => {
                args.push("patch".into());
                args.push(hash.into());
            }
            Self::Diff { hash } => {
                args.push("diff".into());
                args.push(hash.into());
            }
        }
    }
}
