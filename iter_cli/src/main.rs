//! `iter` — composition root binary.
//!
//! This file is intentionally tiny. It parses argv, loads the operator's
//! [`TracingPreferences`](crate::tracing_preferences::TracingPreferences),
//! forks a detached child if `--detach` was passed to the `iter run`
//! subcommand, and otherwise hands off to the matching dispatch function in
//! [`mod@dispatch`].
//!
//! All errors flow through [`output::run_main`] so the user never sees
//! Rust's runtime `Debug` printer.

#![deny(rust_2018_idioms)]
// Single-binary crate: `pub` items in private modules are intentionally
// "unreachable" from outside, but clippy's pedantic preset still flags
// them. The cost of churn-converting every internal `pub` to `pub(crate)`
// across `cli.rs` / `dispatch/*.rs` / module re-exports is not worth it
// for a leaf application.

mod cli;
mod dispatch;
mod naming;
mod output;
mod process;
mod telemetry;
mod tracing_preferences;

// Composition layer absorbed from the former `iter_compose` crate. compose
// differs from `iter run` in cardinality, not kind, so the operator owns both
// (R11): turning an `iter_language` definition into a running `Runner`, and
// managing the compose run on top of those iter processes. Each module keeps
// its concept name; within the CLI this stays a module boundary.
mod agent;
mod arg;
mod compose;
mod discovery;
mod events;
mod iterfile;
mod process_lifecycle;
mod project;
mod project_lock;
mod prompt;
mod queue;
mod secrets;
mod shell_action;
mod source;
mod start;
mod workspace;

pub(crate) use agent::agent_from_def;
pub(crate) use compose::{
    CompletedServices, CompletedTask, ComposeError, ComposePlan, DEFAULT_COMPOSE_FILE,
    FailurePolicy, LABEL_ORCHESTRATOR_BOOT_ID, LABEL_ORCHESTRATOR_PID,
    LABEL_ORCHESTRATOR_START_TIME, LABEL_PROJECT, LABEL_SERVICE, OrchestratorContext,
    TargetedSpawnError, TriggerLifecycleState, TriggerRunError, TriggerStatus, build,
    is_compose_filename, load_compose, read_trigger_status, run, spawn_targeted_service,
    trigger_state_dir, trigger_state_root,
};
mod runner_policy;
pub(crate) use discovery::{
    ActiveOrchestrator, DiscoveryError, ProjectMember, find_active_orchestrator,
    list_all_members_by_project, list_project_members, open_default_registry,
};
pub(crate) use events::{register_event_actions, register_event_actions_from_events};
pub(crate) use process_lifecycle::{
    AdoptedProcessStartError, RunRecordMetadata, bootstrap_adopted, derive_finalize_reason,
};
pub(crate) use project::{ENV_PROJECT_NAME, ProjectSlugError, SlugValidationError, project_slug};
pub(crate) use project_lock::{ProjectLock, ProjectLockError, acquire_project_lock};
pub(crate) use prompt::{build_prompt_selector, prompt_selector_from_defs};
pub(crate) use queue::{QueueBuildError, queue_address, queue_from_def};
pub(crate) use runner_policy::runner_policy_from_def;
pub(crate) use secrets::resolve_secret;
pub(crate) use start::StartError;
pub(crate) use workspace::workspaces_from_def;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::process::{DetachedSpec, ProcessError, ProcessRegistry, SpawnError, spawn_detached};
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use clap_complete::generate;
use thiserror::Error;

use crate::cli::{
    Cli, Command, ComposeCmd, EnqueueArgs, InspectArgs, LogsArgs, ProcessCmd, PsArgs, RunArgs,
    ShellArg, SignalCmd, TargetArgs,
};
use crate::dispatch::{
    AttachError, ComposeCmdError, ComposeUpError, EnqueueCmdError, ProcessCmdError, RunCmdError,
    ValidateCmdError, attach, run_compose_config, run_compose_down, run_compose_ls, run_compose_ps,
    run_compose_up, run_compose_validate, run_discard, run_enqueue, run_inspect, run_kill,
    run_logs, run_promote, run_ps, run_rm, run_run, run_stop, run_validate, status_exit_code,
};
use crate::naming::default_process_name;
use crate::output::{IntoExitCode, cli_println, exit_codes, run_main};
use crate::tracing_preferences::{TracingPreferences, TracingPreferencesError};

#[derive(Debug, Error)]
enum IterCliError {
    #[error("loading tracing preferences: {0}")]
    TracingPreferences(#[source] TracingPreferencesError),
    #[error("building tokio runtime: {0}")]
    Runtime(#[source] io::Error),
    #[error("opening process registry: {0}")]
    OpenRegistry(#[source] ProcessError),
    #[error("locating current executable: {0}")]
    CurrentExe(#[source] io::Error),
    #[error("iterfile not found at {}", path.display())]
    IterfileMissing { path: PathBuf },
    #[error("canonicalising iterfile path {}: {source}", path.display())]
    CanonicaliseIterfile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("spawning detached process: {0}")]
    Spawn(#[source] SpawnError),
    #[error(transparent)]
    Attach(#[from] AttachError),
    #[error(transparent)]
    Run(#[from] RunCmdError),
    #[error(transparent)]
    ComposeUp(#[from] ComposeUpError),
    #[error(transparent)]
    Compose(#[from] ComposeCmdError),
    #[error(transparent)]
    Validate(#[from] ValidateCmdError),
    #[error(transparent)]
    Process(#[from] ProcessCmdError),
    #[error(transparent)]
    Enqueue(#[from] EnqueueCmdError),
}

impl IntoExitCode for IterCliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::TracingPreferences(_) => exit_codes::CONFIG,
            // `CanonicaliseIterfile` lands here, not in `IterfileMissing`,
            // because `canonicalize` only runs after the existence check
            // passed: a failure here means a TOCTOU race or a permission
            // flip, which is an I/O fault. Symmetric with the
            // `RunCmdError::CanonicaliseIterfile` arm in
            // `dispatch::run::iterfile_error_exit_code`.
            Self::Runtime(_) | Self::Spawn(_) | Self::CanonicaliseIterfile { .. } => {
                exit_codes::RUNTIME
            }
            Self::OpenRegistry(_) | Self::CurrentExe(_) => exit_codes::INTERNAL,
            Self::IterfileMissing { .. } => exit_codes::USER_INPUT,
            Self::Attach(e) => e.exit_code(),
            Self::Run(e) => e.exit_code(),
            Self::ComposeUp(e) => e.exit_code(),
            Self::Compose(e) => e.exit_code(),
            Self::Validate(e) => e.exit_code(),
            Self::Process(e) => e.exit_code(),
            Self::Enqueue(e) => e.exit_code(),
        }
    }
}

fn main() -> ! {
    run_main(real_main)
}

fn real_main() -> Result<(), IterCliError> {
    let cli = Cli::parse();
    let prefs = load_tracing_preferences_for(&cli)?;

    match cli.command {
        Command::Run(args) => run_run_dispatch(args, prefs),

        Command::Compose { cmd } => match cmd {
            ComposeCmd::Up(args) => block_on(async move {
                run_compose_up(args, prefs)
                    .await
                    .map_err(IterCliError::ComposeUp)
            }),
            ComposeCmd::Validate(args) => {
                run_compose_validate(&args).map_err(|e| IterCliError::Compose(e.into()))
            }
            ComposeCmd::Config(args) => {
                run_compose_config(&args).map_err(|e| IterCliError::Compose(e.into()))
            }
            ComposeCmd::Ls(args) => {
                run_compose_ls(&args).map_err(|e| IterCliError::Compose(e.into()))
            }
            ComposeCmd::Ps(args) => {
                run_compose_ps(&args).map_err(|e| IterCliError::Compose(e.into()))
            }
            ComposeCmd::Down(args) => block_on(async move {
                run_compose_down(&args)
                    .await
                    .map_err(|e| IterCliError::Compose(e.into()))
            }),
        },

        Command::Validate { path, format } => {
            run_validate(path.as_deref(), format).map_err(IterCliError::from)
        }

        Command::Ps(args) => ps_dispatch(args),
        Command::Logs(args) => logs_dispatch(args),
        Command::Stop(args) => stop_dispatch(args),
        Command::Kill(args) => kill_dispatch(args),
        Command::Rm(args) => rm_dispatch(args),
        Command::Promote(args) => promote_dispatch(args),
        Command::Discard(args) => discard_dispatch(args),
        Command::Inspect(args) => inspect_dispatch(args),
        Command::Enqueue(args) => enqueue_dispatch(args, prefs),

        Command::Process { cmd } => match cmd {
            ProcessCmd::Ls(args) => ps_dispatch(args),
            ProcessCmd::Inspect(args) => inspect_dispatch(args),
            ProcessCmd::Logs(args) => logs_dispatch(args),
            ProcessCmd::Run(args) => run_run_dispatch(args, prefs),
            ProcessCmd::Stop(args) => stop_dispatch(args),
            ProcessCmd::Kill(args) => kill_dispatch(args),
            ProcessCmd::Rm(args) => rm_dispatch(args),
            ProcessCmd::Promote(args) => promote_dispatch(args),
            ProcessCmd::Discard(args) => discard_dispatch(args),
        },

        Command::Signal { cmd } => match cmd {
            SignalCmd::Push(args) => enqueue_dispatch(args, prefs),
        },

        Command::Completions { shell } => {
            emit_completions(shell);
            Ok(())
        }
    }
}

fn ps_dispatch(args: PsArgs) -> Result<(), IterCliError> {
    block_on(async move { run_ps(args).await.map_err(IterCliError::from) })
}

fn logs_dispatch(args: LogsArgs) -> Result<(), IterCliError> {
    block_on(async move { run_logs(args).await.map_err(IterCliError::from) })
}

fn stop_dispatch(args: TargetArgs) -> Result<(), IterCliError> {
    block_on(async move { run_stop(args).await.map_err(IterCliError::from) })
}

fn kill_dispatch(args: TargetArgs) -> Result<(), IterCliError> {
    block_on(async move { run_kill(args).await.map_err(IterCliError::from) })
}

fn rm_dispatch(args: TargetArgs) -> Result<(), IterCliError> {
    block_on(async move { run_rm(args).await.map_err(IterCliError::from) })
}

fn promote_dispatch(args: TargetArgs) -> Result<(), IterCliError> {
    block_on(async move { run_promote(args).await.map_err(IterCliError::from) })
}

fn discard_dispatch(args: TargetArgs) -> Result<(), IterCliError> {
    block_on(async move { run_discard(args).await.map_err(IterCliError::from) })
}

fn inspect_dispatch(args: InspectArgs) -> Result<(), IterCliError> {
    block_on(async move { run_inspect(args).await.map_err(IterCliError::from) })
}

fn enqueue_dispatch(args: EnqueueArgs, prefs: TracingPreferences) -> Result<(), IterCliError> {
    block_on(async move { run_enqueue(args, prefs).await.map_err(IterCliError::from) })
}

fn run_run_dispatch(args: RunArgs, prefs: TracingPreferences) -> Result<(), IterCliError> {
    // Adopted children (`--process-id <ULID>`) take the in-process path
    // unconditionally: they *are* the spawned subprocess, so going through
    // `spawn_detached` again would fork an infinite chain.
    if args.process_id.is_some() {
        return block_on(async move {
            Box::pin(run_run(args, prefs))
                .await
                .map_err(IterCliError::from)
        });
    }
    // Both foreground and `--detach` go through the subprocess spawn path so
    // the resulting record is identical from the registry's point of view
    // (captured stdio, name lock, meta.json). The only difference is what
    // the parent does next: detached returns immediately; foreground attaches
    // to the child's captured stdio + status until the child reaches a
    // terminal state.
    let detach = args.detach;
    block_on(async move {
        let id = spawn_child(args, "run").await?;
        if detach {
            cli_println!("{id}");
            return Ok(());
        }
        let status = attach(id).await?;
        let code = status_exit_code(status);
        if code != 0 {
            std::process::exit(code);
        }
        Ok(())
    })
}

/// Locate the preferences file for whichever subcommand was selected. Only
/// `iter run` (or `iter process run`) exposes `--config` today.
///
/// Adopted children (`iter run --process-id <ULID>`) are an explicit
/// exception: a parse/read failure here would `?`-exit the child
/// before [`crate::iterfile::handle`] reaches `bootstrap_adopted`,
/// leaving the parent-allocated registry record dangling `Initializing`
/// until bootstrap-grace reconciles it. Since the preferences are consumed
/// only by the telemetry layer (their sole field is `log_level`), we degrade
/// to [`TracingPreferences::default`] with a warning to stderr. Detached
/// children have stderr bound to `/dev/null` at this point — the warning is
/// only surfaced once the runtime is up and the tracing subscriber
/// fans into `<dir>/log.ndjson`, so a parse warning emitted
/// before that is intentionally silent. Foreground runs keep the
/// strict behaviour.
fn load_tracing_preferences_for(cli: &Cli) -> Result<TracingPreferences, IterCliError> {
    let (path, is_adopted_child) = match &cli.command {
        Command::Run(args)
        | Command::Process {
            cmd: ProcessCmd::Run(args),
        } => (args.config.clone(), args.process_id.is_some()),
        _ => (None, false),
    };
    match TracingPreferences::load(path.as_deref()) {
        Ok(prefs) => Ok(prefs),
        Err(e) if is_adopted_child => {
            tracing::warn!(
                error = %e,
                "failed to load tracing preferences for adopted child; using defaults so the parent-allocated record can finalize cleanly"
            );
            Ok(TracingPreferences::default())
        }
        Err(e) => Err(IterCliError::TracingPreferences(e)),
    }
}

/// Build and run the tokio runtime that hosts the dispatchers.
fn block_on<F: Future<Output = Result<(), IterCliError>>>(future: F) -> Result<(), IterCliError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(IterCliError::Runtime)?;
    runtime.block_on(future)
}

/// Spawn a fully detached `iter` child via `crate::process::spawn_detached`
/// and return the freshly-allocated [`crate::process::ProcessId`].
///
/// The child argv is rebuilt from the user-supplied [`RunArgs`] so the
/// canonicalised flag shape is what gets recorded in `meta.json`. The
/// trailing `--detach` is dropped (the child runs inline) and the spawner
/// itself appends `--process-id <ULID>` so the child can adopt the proc
/// directory the parent just registered.
///
/// Caller decides what to do with the returned id: `--detach` prints it
/// and exits, foreground hands it to [`crate::dispatch::attach`].
async fn spawn_child(
    args: RunArgs,
    subcommand: &'static str,
) -> Result<process::ProcessId, IterCliError> {
    let registry = ProcessRegistry::open_default().map_err(IterCliError::OpenRegistry)?;
    let program = std::env::current_exe().map_err(IterCliError::CurrentExe)?;
    let iterfile = canonical_iterfile(args.iterfile.as_deref())?;
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| default_process_name(&iterfile));

    let mut child_args: Vec<String> = subcommand.split_whitespace().map(str::to_owned).collect();
    child_args.push(iterfile.display().to_string());
    if let Some(cfg) = args.config.as_ref() {
        child_args.push("--config".into());
        child_args.push(cfg.display().to_string());
    }
    if args.once {
        child_args.push("--once".into());
    }
    if args.debug {
        child_args.push("--debug".into());
    }
    for entry in &args.arg {
        child_args.push("--arg".into());
        child_args.push(entry.clone());
    }

    let spec = DetachedSpec {
        name,
        iterfile,
        subcommand: subcommand.to_owned(),
        args: child_args,
        program,
        env: Vec::new(),
        debug: args.debug,
        parent_id: None,
        labels: BTreeMap::new(),
    };
    spawn_detached(&registry, spec)
        .await
        .map_err(IterCliError::Spawn)
}

fn canonical_iterfile(path: Option<&Path>) -> Result<PathBuf, IterCliError> {
    let raw = match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("Iterfile"),
    };
    if !raw.exists() {
        return Err(IterCliError::IterfileMissing { path: raw });
    }
    raw.canonicalize()
        .map_err(|source| IterCliError::CanonicaliseIterfile { path: raw, source })
}

/// Render a shell completion script via `clap_complete::generate` and emit
/// it through `cli_println!` so the `BrokenPipe` policy applies.
///
/// Buffering through `Vec<u8>` avoids two failure modes of writing the
/// generator's bytes straight to a locked stdout: completion generators
/// don't surface intermediate write errors, so a closed pipe (`iter
/// completions bash | head -1`) would either panic from inside the
/// generator or silently truncate. With a `Vec<u8>` sink the generator
/// always succeeds, and the single `cli_println!` invocation routes
/// through the shared `BrokenPipe` swallow path.
fn emit_completions(shell: ShellArg) {
    let target = match shell {
        ShellArg::Bash => Shell::Bash,
        ShellArg::Zsh => Shell::Zsh,
        ShellArg::Fish => Shell::Fish,
        ShellArg::Powershell => Shell::PowerShell,
        ShellArg::Elvish => Shell::Elvish,
    };
    let mut cmd = Cli::command();
    let mut buf: Vec<u8> = Vec::new();
    generate(target, &mut cmd, "iter", &mut buf);
    let script = String::from_utf8(buf).expect("clap_complete output is UTF-8");
    // Trailing newline preserved by clap_complete; emit raw without
    // appending another via cli_println!'s implicit "\n".
    let trimmed = script.strip_suffix('\n').unwrap_or(&script);
    cli_println!("{trimmed}");
}
