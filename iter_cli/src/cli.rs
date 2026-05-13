//! `clap` derive structures for the `iter` CLI.
//!
//! Every interaction is a `<resource> <verb>` pair, with a small set of
//! ergonomic top-level aliases (`iter ps`, `iter logs`, `iter inspect`, …)
//! preserved for muscle memory.
//!
//! ```text
//! Canonical:
//!   iter process ls       (alias: iter ps)
//!   iter process inspect  (alias: iter inspect)
//!   iter process logs     (alias: iter logs)
//!   iter process stop     (alias: iter stop)
//!   iter process kill     (alias: iter kill)
//!   iter process rm       (alias: iter rm)
//!   iter process run      (alias: iter run)
//!   iter signal push      (alias: iter enqueue)
//!   iter compose up
//!   iter compose ls       (alias: iter compose ps)
//!   iter compose validate
//!   iter validate <PATH>  (autodetect Iterfile vs compose.iter)
//!   iter completions <SHELL>
//! ```
//!
//! Iterfile authoring tools (linting, formatting, LSP) are out of scope
//! for this binary. Third-party tools should consume `iter_language`
//! directly.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::output::{ListingArgs, ValidateFormat};

/// Top-level `iter` CLI definition.
#[derive(Debug, Parser)]
#[command(
    name = "iter",
    version,
    about = "AI agent control framework",
    long_about = "iter is an agent control framework that turns an Iterfile into \
                  a runnable composition of queue, workspace, agent, and trigger \
                  implementations.\n\n\
                  Resources:  process, signal, compose, iterfile.\n\
                  Verbs:      ls, inspect, run, stop, kill, rm, logs, push, validate.\n\n\
                  Top-level aliases (`iter ps`, `iter logs`, `iter inspect`, …) are \
                  preserved for ergonomics; the canonical form is `iter <resource> <verb>`.",
    after_help = "EXAMPLES:\n  \
                  iter run Iterfile --detach\n  \
                  iter ps -q | xargs iter rm\n  \
                  iter compose up -f compose.iter\n  \
                  iter inspect <ID> | jq .name\n  \
                  iter completions zsh > ~/.zsh/_iter"
)]
pub struct Cli {
    /// Subcommand to dispatch.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
///
/// The canonical names are the resource-noun forms (`process`, `signal`).
/// Top-level verbs (`ps`, `logs`, `stop`, `kill`, `rm`, `inspect`,
/// `enqueue`, `run`) are kept as ergonomic aliases that share their
/// underlying argument structs with the canonical forms.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run an Iterfile in runner-only mode (no triggers).
    #[command(
        long_about = "Run a single Iterfile against its declared queue.\n\n\
                      With `--detach` the process forks into the background and the new \
                      ULID is printed to stdout (and only the ULID — composes with \
                      `ID=$(iter run --detach Iterfile)`).",
        after_help = "EXAMPLES:\n  \
                      iter run Iterfile\n  \
                      iter run Iterfile --detach --name api-poller\n  \
                      iter run --once Iterfile"
    )]
    Run(RunArgs),

    /// Compose orchestration: spin up multiple services / triggers from a
    /// `compose.iter` file.
    Compose {
        /// Nested compose subcommand.
        #[command(subcommand)]
        cmd: ComposeCmd,
    },

    /// Validate an Iterfile or compose.iter and exit with non-zero on the
    /// first error. The file kind is detected from its basename.
    #[command(after_help = "EXAMPLES:\n  \
                      iter validate Iterfile\n  \
                      iter validate compose.iter\n  \
                      iter validate --format json compose.iter")]
    Validate {
        /// Path to the file (defaults to `./Iterfile`).
        path: Option<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value_t = ValidateFormat::Text)]
        format: ValidateFormat,
    },

    /// List detached process records (alias for `iter process ls`).
    #[command(
        long_about = "List detached process records managed by the local registry.\n\n\
                      `-q / --quiet` prints one ULID per line on stdout, composable \
                      with `xargs iter rm`. `--format json` emits one NDJSON record \
                      per process with full ID and ISO-8601 UTC timestamps. \
                      `--no-trunc` disables the 12-character ULID truncation in the \
                      human table view.",
        after_help = "EXAMPLES:\n  \
                      iter ps\n  \
                      iter ps -q | xargs iter rm\n  \
                      iter ps --format json | jq '.id'\n  \
                      iter ps --no-trunc --all"
    )]
    Ps(PsArgs),

    /// Tail the logs of a detached process (alias for `iter process logs`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter logs <ID>\n  \
                      iter logs <ID> --follow")]
    Logs(LogsArgs),

    /// Send `SIGTERM` to a detached process (alias for `iter process stop`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter stop <ID>\n  \
                      iter stop -q <ID>")]
    Stop(TargetArgs),

    /// Send `SIGKILL` to a detached process (alias for `iter process kill`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter kill <ID>\n  \
                      iter kill -q <ID>")]
    Kill(TargetArgs),

    /// Remove a stopped process directory (alias for `iter process rm`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter rm <ID>\n  \
                      iter ps -q | xargs iter rm")]
    Rm(TargetArgs),

    /// Show the metadata document for a process (alias for `iter process inspect`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter inspect <ID>\n  \
                      iter inspect <ID> | jq '.status'")]
    Inspect(InspectArgs),

    /// Push a signal onto a queue (alias for `iter signal push`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter enqueue --queue-url memory://\n  \
                      iter enqueue -f Iterfile --priority high\n  \
                      iter enqueue --queue-url memory:// --metadata source=manual")]
    Enqueue(EnqueueArgs),

    /// Process resource — canonical form for ps / inspect / logs / run / stop / kill / rm.
    Process {
        /// Nested process subcommand.
        #[command(subcommand)]
        cmd: ProcessCmd,
    },

    /// Signal resource — canonical form for `signal push`.
    Signal {
        /// Nested signal subcommand.
        #[command(subcommand)]
        cmd: SignalCmd,
    },

    /// Generate a shell completion script for the named shell.
    #[command(after_help = "EXAMPLES:\n  \
                      source <(iter completions bash)\n  \
                      iter completions zsh  > ~/.zfunc/_iter\n  \
                      iter completions fish > ~/.config/fish/completions/iter.fish")]
    Completions {
        /// Shell flavour to generate completions for.
        shell: ShellArg,
    },
}

/// Subcommands grouped under `iter compose`.
#[derive(Debug, Subcommand)]
pub enum ComposeCmd {
    /// Build and spawn services and triggers declared in `compose.iter`.
    /// When targets are given, only the named services are started
    /// (requires `--detach`).
    #[command(after_help = "EXAMPLES:\n  \
                      iter compose up\n  \
                      iter compose up -f dev.compose.iter\n  \
                      iter compose up --on-failure continue\n  \
                      iter compose up worker-a --detach\n  \
                      iter compose up --source ./worker-a/Iterfile --detach")]
    Up(ComposeUpArgs),
    /// Parse and semantic-check a `compose.iter` and exit.
    #[command(after_help = "EXAMPLES:\n  \
                      iter compose validate\n  \
                      iter compose validate -f dev.compose.iter --format json")]
    Validate(ComposeValidateArgs),
    /// List the queues, services, and triggers declared in `compose.iter`.
    #[command(
        long_about = "List the queues, services, and triggers declared in a compose.iter \
                      file as a `KIND  NAME  DETAIL` table.\n\n\
                      `-q / --quiet` emits one `kind/name` pair per line on stdout, \
                      letting scripts grep for a specific resource (e.g. \
                      `iter compose config -q | grep '^queue/'`). Compose resources do \
                      not have persistent IDs — name alone may collide across kinds, \
                      so the kind prefix is part of the contract.\n\n\
                      `--format json` emits a JSON array of `{kind,name,detail}` \
                      objects.\n\n\
                      Mirrors `docker compose config`. For runtime listings of \
                      active projects and runners see `iter compose ls` and \
                      `iter compose ps`.",
        after_help = "EXAMPLES:\n  \
                      iter compose config\n  \
                      iter compose config -q | grep '^queue/'\n  \
                      iter compose config --format json"
    )]
    Config(ComposeConfigArgs),
    /// List active compose projects (mirrors `docker compose ls`).
    #[command(
        long_about = "List every compose project that has at least one runner \
                      registered in the local process registry, grouped by \
                      `iter.compose.project` label. Stateless: the orchestrator \
                      itself is not registered, so a project appears here as long \
                      as one of its services has been spawned.",
        after_help = "EXAMPLES:\n  \
                      iter compose ls\n  \
                      iter compose ls --format json"
    )]
    Ls(ComposeLsArgs),
    /// List runners belonging to a single compose project (mirrors
    /// `docker compose ps`).
    #[command(
        long_about = "List runners belonging to one compose project. The project \
                      is derived from the compose file's parent directory \
                      basename, overridable with `-p / --project-name` or via \
                      `COMPOSE_PROJECT_NAME`. Filters the local process registry \
                      by `iter.compose.project` label.",
        after_help = "EXAMPLES:\n  \
                      iter compose ps\n  \
                      iter compose ps -f dev.compose.iter\n  \
                      iter compose ps -p my-project --format json"
    )]
    Ps(ComposePsArgs),
    /// Stop runners in a compose project (mirrors `docker compose down`).
    /// When targets are given, only the named services are stopped;
    /// the orchestrator and siblings are left running.
    #[command(
        long_about = "Send `SIGTERM` to every runner in a compose project, then \
                      to the orchestrator process discovered through any \
                      runner's `iter.compose.orchestrator_pid` label. Escalates \
                      to `SIGKILL` after `--timeout` seconds (default 30).\n\n\
                      When one or more service targets are given, only those \
                      services are stopped; the orchestrator and sibling \
                      services are left running.",
        after_help = "EXAMPLES:\n  \
                      iter compose down\n  \
                      iter compose down -f dev.compose.iter\n  \
                      iter compose down -p my-project --timeout 5\n  \
                      iter compose down worker-a\n  \
                      iter compose down --source ./worker-a/Iterfile"
    )]
    Down(ComposeDownArgs),
}

/// Subcommands grouped under `iter process`.
#[derive(Debug, Subcommand)]
pub enum ProcessCmd {
    /// List detached process records.
    #[command(
        visible_alias = "ps",
        after_help = "EXAMPLES:\n  \
                      iter process ls\n  \
                      iter process ls -q | xargs iter process rm\n  \
                      iter process ls --format json | jq '.id'"
    )]
    Ls(PsArgs),
    /// Show the metadata document for a process.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process inspect <ID>\n  \
                      iter process inspect <ID> | jq '.status'")]
    Inspect(InspectArgs),
    /// Tail the logs of a detached process.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process logs <ID>\n  \
                      iter process logs <ID> --follow")]
    Logs(LogsArgs),
    /// Run an Iterfile.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process run Iterfile\n  \
                      iter process run Iterfile --detach --name api-poller\n  \
                      iter process run --once Iterfile")]
    Run(RunArgs),
    /// Send `SIGTERM` to a detached process.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process stop <ID>\n  \
                      iter process stop -q <ID>")]
    Stop(TargetArgs),
    /// Send `SIGKILL` to a detached process.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process kill <ID>\n  \
                      iter process kill -q <ID>")]
    Kill(TargetArgs),
    /// Remove a stopped process directory.
    #[command(after_help = "EXAMPLES:\n  \
                      iter process rm <ID>\n  \
                      iter process ls -q | xargs iter process rm")]
    Rm(TargetArgs),
}

/// Subcommands grouped under `iter signal`.
#[derive(Debug, Subcommand)]
pub enum SignalCmd {
    /// Push a single signal onto a queue.
    #[command(after_help = "EXAMPLES:\n  \
                      iter signal push --queue-url memory://\n  \
                      iter signal push -f Iterfile --priority high\n  \
                      iter signal push --queue-url memory:// --metadata source=manual")]
    Push(EnqueueArgs),
}

/// Arguments accepted by `iter compose validate`.
#[derive(Debug, Parser, Clone)]
pub struct ComposeValidateArgs {
    /// Path to the compose file (defaults to `./compose.iter`).
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = ValidateFormat::Text)]
    pub format: ValidateFormat,
}

/// Arguments accepted by `iter compose config`.
#[derive(Debug, Parser, Clone)]
pub struct ComposeConfigArgs {
    /// Path to the compose file (defaults to `./compose.iter`).
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Shared `-q`, `--format`, `--no-trunc` listing flags.
    #[command(flatten)]
    pub listing: ListingArgs,
}

/// Arguments accepted by `iter compose ls`. No `-f` — this enumerates
/// every active project from the local registry.
#[derive(Debug, Parser, Clone)]
pub struct ComposeLsArgs {
    /// Include projects whose runners are all in a terminal state.
    /// Defaults to false — like `docker compose ls`, only running
    /// projects are listed.
    #[arg(short = 'a', long = "all", default_value_t = false)]
    pub all: bool,

    /// Shared `-q`, `--format`, `--no-trunc` listing flags.
    #[command(flatten)]
    pub listing: ListingArgs,
}

/// Arguments accepted by `iter compose ps`.
#[derive(Debug, Parser, Clone)]
pub struct ComposePsArgs {
    /// Path to the compose file used to derive the project slug
    /// (defaults to `./compose.iter`). Ignored when `-p` is given.
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Project name override (docker-compose convention). Takes
    /// precedence over the compose file's directory basename.
    #[arg(short = 'p', long = "project-name")]
    pub project_name: Option<String>,

    /// Include runners in a terminal state. Defaults to false — like
    /// `docker compose ps`, only non-terminal runners are listed.
    #[arg(short = 'a', long = "all", default_value_t = false)]
    pub all: bool,

    /// Shared `-q`, `--format`, `--no-trunc` listing flags.
    #[command(flatten)]
    pub listing: ListingArgs,
}

/// Arguments accepted by `iter compose down`.
#[derive(Debug, Parser, Clone)]
pub struct ComposeDownArgs {
    /// Optional service targets. When given, only the named services are
    /// stopped; the orchestrator and sibling services are left running.
    /// Accepts bare service names (`worker-a`) or explicit resource
    /// references (`service/worker-a`).
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Path to the compose file used to derive the project slug
    /// (defaults to `./compose.iter`). Ignored when `-p` is given.
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Project name override (docker-compose convention). Takes
    /// precedence over the compose file's directory basename.
    #[arg(short = 'p', long = "project-name")]
    pub project_name: Option<String>,

    /// Stop services built from the given Iterfile path. Resolved
    /// against `service { build = ... }` declarations in the compose
    /// file. If multiple services share the same Iterfile, all are
    /// stopped.
    #[arg(long = "source")]
    pub source: Option<PathBuf>,

    /// Seconds to wait for graceful `SIGTERM` shutdown before
    /// escalating to `SIGKILL`. Mirrors `docker compose down --timeout`.
    #[arg(short = 't', long = "timeout", default_value_t = 30)]
    pub timeout: u64,

    /// Suppress per-runner status lines.
    #[arg(short, long)]
    pub quiet: bool,
}

/// Arguments accepted by `iter compose up`.
#[derive(Debug, Parser, Clone)]
pub struct ComposeUpArgs {
    /// Optional service targets. When given, only the named services are
    /// started. Requires `--detach`. Accepts bare service names
    /// (`worker-a`) or explicit resource references (`service/worker-a`).
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Path to the compose file (defaults to `./compose.iter`).
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// What to do when one task fails.
    #[arg(long = "on-failure", value_enum, default_value_t = ComposeFailure::Abort)]
    pub on_failure: ComposeFailure,

    /// Run the compose orchestrator in the background. The launching shell
    /// returns immediately; the orchestrator hosts triggers in-process and
    /// runner services keep their usual `~/.iter/proc/<id>/` records.
    /// Unlike `iter run --detach`, the orchestrator itself is **not**
    /// registered (compose is stateless, like `docker compose`).
    #[arg(short, long)]
    pub detach: bool,

    /// Project name override. Defaults to the canonical basename of the
    /// compose file's parent directory (docker-compose convention).
    /// Same project name = same project; use this when two compose files
    /// in different directories happen to share a basename.
    #[arg(short = 'p', long = "project-name")]
    pub project_name: Option<String>,

    /// Start services built from the given Iterfile path. Resolved
    /// against `service { build = ... }` declarations in the compose
    /// file. Requires `--detach`.
    #[arg(long = "source")]
    pub source: Option<PathBuf>,

    /// Enable debug-level tracing output.
    #[arg(long)]
    pub debug: bool,
}

/// Mirror of [`iter_compose::FailurePolicy`] for clap parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ComposeFailure {
    /// Cancel every other task on the first error.
    Abort,
    /// Log the failure and let the surviving tasks run to completion.
    Continue,
}

/// Arguments accepted by `iter run` / `iter process run`.
#[derive(Debug, Parser, Clone)]
pub struct RunArgs {
    /// Path to the Iterfile (defaults to `./Iterfile`).
    pub iterfile: Option<PathBuf>,

    /// Path to the optional TOML config file (defaults to
    /// `~/.iter/config.toml`).
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Spawn a detached background process and return its instance id.
    ///
    /// Mac/Linux only. On Windows this returns a clean "not supported"
    /// error.
    #[arg(short, long)]
    pub detach: bool,

    /// Human-friendly name to assign to the spawned instance.
    #[arg(long)]
    pub name: Option<String>,

    /// Exit after exactly one signal has been processed.
    #[arg(long)]
    pub once: bool,

    /// Enable debug-level tracing output.
    #[arg(long)]
    pub debug: bool,

    /// Internal: set by `iter_core::process::spawn_detached` when forking
    /// the detached child. Triggers `adopt_from_argv` instead of
    /// `register_foreground`.
    #[arg(long = "process-id", hide = true)]
    pub process_id: Option<String>,

    /// Run a single named service from a compose file instead of an
    /// Iterfile.
    ///
    /// When set, the positional path is interpreted as a `compose.iter`
    /// file and only the named service is built and run; sibling
    /// services and triggers in the file are ignored. The compose
    /// orchestrator (`iter compose up`) uses this internally to spawn
    /// each service as its own subprocess so that every service shows
    /// up in `iter ps` / `iter logs` independently.
    #[arg(long = "service")]
    pub service: Option<String>,

    /// Override an Iterfile `arg` default. Repeatable.
    ///
    /// Format: `--arg key=value`. Overrides the `arg <key> = "<default>"`
    /// declaration in the Iterfile. If the Iterfile declares `arg <key>`
    /// with no default, the override is required.
    #[arg(long = "arg", value_name = "KEY=VALUE")]
    pub arg: Vec<String>,
}

/// Arguments accepted by `iter enqueue` / `iter signal push`.
#[derive(Debug, Parser, Clone)]
pub struct EnqueueArgs {
    /// Connect-style queue URL (`file:///abs/path`, `memory://`, `redis://...`).
    /// Takes precedence over `-f`.
    #[arg(long = "queue-url")]
    pub queue_url: Option<String>,

    /// Path to a `compose.iter` or `Iterfile`. When omitted, the file is
    /// auto-detected from the working directory (`./compose.iter` →
    /// `./Iterfile`).
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Queue name when the resolved file declares more than one queue.
    /// Compose-only.
    #[arg(long = "queue")]
    pub queue: Option<String>,

    /// Metadata pair `KEY=VALUE`. Repeatable. Values are stored as strings.
    #[arg(short = 'm', long = "metadata", value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,

    /// Signal priority.
    #[arg(long = "priority", value_enum, default_value_t = EnqueuePriority::Normal)]
    pub priority: EnqueuePriority,
}

/// Mirror of [`iter_core::Priority`] for clap parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EnqueuePriority {
    /// Background work that may be deferred indefinitely.
    Low,
    /// Default priority used when no value is supplied.
    Normal,
    /// Foreground work that should be processed promptly.
    High,
    /// Critical work that should preempt anything else.
    Critical,
}

/// Arguments accepted by `iter ps` / `iter process ls`.
#[derive(Debug, Parser, Clone)]
pub struct PsArgs {
    /// Show stopped/failed/killed instances in addition to running ones.
    #[arg(short, long)]
    pub all: bool,

    /// Shared `-q`, `--format`, `--no-trunc` listing flags.
    #[command(flatten)]
    pub listing: ListingArgs,
}

/// Arguments accepted by `iter logs` / `iter process logs`.
#[derive(Debug, Parser, Clone)]
pub struct LogsArgs {
    /// Instance id (ulid) or human-friendly name.
    pub instance: String,

    /// Follow new lines as they arrive (`tail -f`).
    #[arg(short, long)]
    pub follow: bool,

    /// Print only the last N lines before following.
    #[arg(long)]
    pub tail: Option<usize>,

    /// Show RFC3339 microsecond timestamps in front of every line, matching
    /// the `docker logs -t` shape.
    #[arg(short = 't', long)]
    pub timestamps: bool,
}

/// Arguments accepted by `iter inspect` / `iter process inspect`.
///
/// `inspect` is JSON-only by design — it is the source of truth for a
/// resource (P8). Tabular views belong on `iter ps` / `iter process ls`.
/// Deliberately no `--format` flag here so clap cannot accept a value
/// the dispatcher would only reject.
#[derive(Debug, Parser, Clone)]
pub struct InspectArgs {
    /// Instance id (ulid) or human-friendly name.
    pub instance: String,
}

/// Generic single-target arguments (stop / kill / rm).
#[derive(Debug, Parser, Clone)]
pub struct TargetArgs {
    /// Instance id (ulid) or human-friendly name.
    pub instance: String,

    /// Suppress the "<id>: <from> -> <to>" confirmation on stderr.
    /// Successful exit (`0`) is the success signal under `--quiet`.
    #[arg(short, long, default_value_t = false)]
    pub quiet: bool,
}

/// Shell choices accepted by `iter completions <SHELL>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ShellArg {
    /// Generate a Bash completion script.
    Bash,
    /// Generate a Zsh completion script.
    Zsh,
    /// Generate a Fish completion script.
    Fish,
    /// Generate a `PowerShell` completion script.
    Powershell,
    /// Generate an Elvish completion script.
    Elvish,
}
