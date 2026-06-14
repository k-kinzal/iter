//! `iter run` handler — turn a parsed Iterfile into a running [`Runner`]
//! and bind it to an on-disk process record.
//!
//! This module is the iter-side counterpart to the Clap subcommand layer
//! in `iter_cli`. [`handle`] takes a [`RunInput`] describing what to run
//! (iterfile, `--once` semantics, foreground vs adopted mode, metadata to
//! record) and drives the runner to completion, finalising the process
//! registry entry with a derived
//! [`ProcessTerminationReason`](crate::process_lifecycle::ProcessTerminationReason).
//!
//! The Clap layer is responsible for argv parsing, default-name
//! resolution, telemetry init, and translating its argument struct into
//! [`RunInput`]; this module knows nothing about argv shape or CLI
//! flags.
//!
//! See [`crate::compose`] for the analogous handler that runs a
//! `compose.iter` plan.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iter_core::process::{
    AdoptError, ProcessError, ProcessId, ProcessRuntime, install_signal_handlers,
};
use iter_core::{BuilderError, RunnerExitError, RunnerSummary};
use iter_language::{Diagnostic, Iterfile, parse};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::arg::{ArgError, resolve_args};
use crate::compose::{ComposeError, build_single_service, load_compose};
use crate::process_lifecycle::{
    self, AdoptedBootstrapError, LifecycleError, TerminationRecorder, derive_finalize_reason,
    leaves_record_non_terminal, log_finalize_report, terminal_status_for,
};
use crate::queue::QueueBuildError;
use crate::source::{ActiveSource, SourceBuildError};
use crate::start::{self, StartError};

pub use crate::process_lifecycle::RunRecordMetadata;

/// Input passed to [`handle`].
pub struct RunInput {
    /// Path to the Iterfile (may be relative; the handler canonicalises
    /// it after adoption so a missing/invalid path on the adopted path
    /// still flips the record to a terminal state).
    ///
    /// For [`RunSource::ComposeService`] this points at the compose file
    /// instead of an Iterfile; the handler will parse it as compose and
    /// extract the named service.
    pub iterfile_path: PathBuf,
    /// What kind of source [`iterfile_path`] is.
    pub source: RunSource,
    /// `--once` semantics: exit after one signal has been processed.
    pub once: bool,
    /// Foreground vs adopted mode.
    pub mode: RunMode,
    /// Bookkeeping recorded into the process registry entry.
    pub metadata: RunRecordMetadata,
    /// CLI `--arg key=value` overrides for Iterfile arg declarations.
    pub arg_overrides: BTreeMap<String, String>,
}

/// What [`RunInput::iterfile_path`] points at.
pub enum RunSource {
    /// A plain Iterfile (default). The handler parses it directly and
    /// builds a single Runner from the queue/workspace/agent/runner
    /// declarations at the top level.
    Iterfile,
    /// A compose file containing one or more services. The handler
    /// parses it as compose and runs only the named service. Used by
    /// the compose orchestrator when it spawns each service as a
    /// subprocess (`iter run --service NAME -f compose.iter`).
    ComposeService {
        /// Name of the service to run (must match a `service NAME { ... }`
        /// or `service NAME from "...iter"` declaration in the compose
        /// file).
        service_name: String,
    },
}

/// How [`handle`] integrates with the on-disk process registry.
pub enum RunMode {
    /// Brand-new run; if registration succeeds, the run is recorded as a
    /// foreground process. Registry failures (e.g. read-only `$HOME`)
    /// are tolerated and yield a record-less run.
    Foreground {
        /// Human-friendly name to assign to the process record.
        name: String,
    },
    /// Adopt a parent-allocated record set up by `spawn_detached`. The
    /// parent allocated the record and bound the child's fd 1/2 to
    /// `/dev/null` before exec; the child opens `<dir>/log.ndjson`
    /// itself via the in-process [`OutputPolicy::LogOnly`] wiring.
    Adopted {
        /// Process id allocated by the parent.
        process_id: ProcessId,
    },
}

/// Errors produced by [`handle`] / `run_inner` while loading and running
/// an Iterfile.
#[derive(Debug, Error)]
pub enum IterfileError {
    /// Canonicalising the iterfile path failed.
    #[error("canonicalising iterfile path {}: {source}", path.display())]
    Canonicalise {
        /// Offending path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Reading the iterfile from disk failed.
    #[error("reading iterfile at {}: {source}", path.display())]
    Read {
        /// Offending path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The parser produced one or more error-severity diagnostics.
    #[error("{rendered}")]
    Parse {
        /// Pre-rendered diagnostic text.
        rendered: String,
    },
    /// A required section is missing from the iterfile.
    #[error("iterfile is missing the `{0}` section")]
    MissingSection(&'static str),
    /// Arg resolution failed (missing required arg, unknown override, or
    /// render error).
    #[error(transparent)]
    Arg(#[from] ArgError),
    /// Building a queue declaration failed.
    #[error(transparent)]
    QueueBuild(#[from] QueueBuildError),
    /// Building the runner builder from the plan failed (agent, prompt,
    /// or event handler).
    #[error(transparent)]
    Start(#[from] StartError),
    /// Source provisioning, disposition, or pending-decision recording failed.
    #[error(transparent)]
    Source(#[from] SourceBuildError),
    /// `RunnerBuilder::build` rejected the wired configuration.
    #[error(transparent)]
    Builder(#[from] BuilderError),
    /// The runner exited with an error.
    #[error(transparent)]
    Runner(#[from] RunnerExitError),
    /// Opening the process registry for the adopted child failed.
    #[error("opening process registry: {0}")]
    RegistryOpen(#[source] ProcessError),
    /// Adopting a parent-allocated process record failed.
    #[error("adopting process {id}: {source}")]
    Adopt {
        /// Adopted process id.
        id: ProcessId,
        /// Underlying error.
        #[source]
        source: AdoptError,
    },
    /// Foreground process registry bootstrap failed.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    /// The runner finished cleanly but the registry record could not be
    /// flipped to a terminal state.
    #[error("finalize failed to write terminal status: {0}")]
    FinalizeStatus(#[source] ProcessError),
    /// `--service NAME` was given but the compose file did not declare
    /// such a service.
    #[error("compose file {} has no service named `{name}`", path.display())]
    UnknownService {
        /// Compose file path.
        path: PathBuf,
        /// Requested service name.
        name: String,
    },
    /// Loading or building the compose plan failed under
    /// [`RunSource::ComposeService`].
    #[error(transparent)]
    Compose(#[from] Box<ComposeError>),
}

/// Run an Iterfile to completion, optionally bound to a process record.
///
/// Adopted runs adopt their parent-allocated record before any fallible
/// step — including iterfile loading — so a syntax error or missing
/// path still flips the record to a terminal state via the
/// finalize-on-return path. Otherwise the parent's `Initializing` entry
/// would dangle until bootstrap-grace expires. Foreground runs register
/// themselves only after iterfile validation succeeds.
///
/// Whichever runtime ends up bound to the run is finalised once on the
/// outer return path with a reason derived from the runner result and
/// any shutdown signal that fired.
///
/// # Errors
///
/// Returns an error when registry adoption fails, when the iterfile
/// cannot be read or parsed, when a required section is missing, when
/// runner construction fails, or when the runner itself exits with an
/// error.
pub async fn handle(input: RunInput) -> Result<(), IterfileError> {
    let mut runtime: Option<(ProcessRuntime, TerminationRecorder)> = match &input.mode {
        RunMode::Adopted { process_id } => Some(bootstrap_adopted(*process_id).await?),
        RunMode::Foreground { .. } => None,
    };

    let run_result = run_inner(&input, &mut runtime).await;

    let finalize_err = if let Some((rt, termination)) = runtime {
        let failure_msg = run_result.as_ref().err().map(ToString::to_string);
        let reason = derive_finalize_reason(failure_msg, &termination);
        let report = rt.finalize(terminal_status_for(&reason)).await;
        log_finalize_report(&report);
        report.status_write_error.filter(leaves_record_non_terminal)
    } else {
        None
    };

    match (run_result, finalize_err) {
        (Ok(_), None) => Ok(()),
        (Err(runner_err), _) => Err(runner_err),
        // Runner succeeded but the registry is left non-terminal — surface
        // the finalize failure so the caller (and the user) sees that the
        // record needs manual cleanup instead of treating the run as a
        // clean exit.
        (Ok(_), Some(finalize_err)) => Err(IterfileError::FinalizeStatus(finalize_err)),
    }
}

async fn run_inner(
    input: &RunInput,
    runtime: &mut Option<(ProcessRuntime, TerminationRecorder)>,
) -> Result<RunnerSummary, IterfileError> {
    let canonical_path =
        input
            .iterfile_path
            .canonicalize()
            .map_err(|source| IterfileError::Canonicalise {
                path: input.iterfile_path.clone(),
                source,
            })?;

    // Build the runner builder + the registry-record path (which may
    // differ from `canonical_path` for compose-service builds whose
    // service points at a separate `build = ...` Iterfile).
    let (mut builder, record_path, active_source) = match &input.source {
        RunSource::Iterfile => {
            build_iterfile_builder(&canonical_path, input.once, &input.arg_overrides).await?
        }
        RunSource::ComposeService { service_name } => {
            let (builder, path) =
                build_compose_service_builder(&canonical_path, service_name, input.once)?;
            (builder, path, None)
        }
    };

    if runtime.is_none()
        && let RunMode::Foreground { name } = &input.mode
    {
        *runtime =
            process_lifecycle::bootstrap_foreground(name, &record_path, &input.metadata, None)
                .await?;
    }

    if let Some((rt, _)) = runtime.as_ref() {
        builder = start::wire_builder_runtime(builder, rt);
        if let Some(sender) = rt.log_sender() {
            iter_core::process::install_global_log_sender(sender);
        }
    }
    let runner = builder.build()?;

    info!(
        iterfile = %record_path.display(),
        once = input.once,
        "starting runner"
    );

    let runner_result = if let Some((rt, _)) = runtime.as_ref() {
        runner.run(rt.shutdown().token()).await
    } else {
        // Record-less foreground still needs SIGINT/SIGTERM to
        // cancel the runner cleanly; the token-only interrupt is
        // enough because there is no record to classify.
        let cancel = install_signal_handlers(CancellationToken::new()).map_err(|source| {
            IterfileError::Lifecycle(LifecycleError::InstallSignalHandlers(source))
        })?;
        runner.run(cancel).await
    };

    let summary = match runner_result {
        Ok(summary) => {
            info!(
                iterations = summary.iteration_count,
                last = ?summary.last_signal_id,
                reason = ?summary.termination_reason,
                "runner exited"
            );
            summary
        }
        Err(err) => {
            error!(error = %err, "runner exited with error");
            return Err(IterfileError::Runner(err));
        }
    };

    dispose_active_source(active_source, runtime.as_ref()).await?;
    Ok(summary)
}

/// Build a [`RunnerBuilder`](iter_core::RunnerBuilder)-shaped value from
/// a plain Iterfile.
///
/// Returns the builder along with the canonical iterfile path that the
/// caller will record into the per-process registry entry.
async fn build_iterfile_builder(
    iterfile_path: &Path,
    once: bool,
    arg_overrides: &BTreeMap<String, String>,
) -> Result<(iter_core::RunnerBuilder, PathBuf, Option<ActiveSource>), IterfileError> {
    let mut iterfile = load_and_parse(iterfile_path)?;
    resolve_args(&mut iterfile, arg_overrides)?;

    let runner = iterfile
        .runners
        .first()
        .ok_or(IterfileError::MissingSection("runner"))?;

    if iterfile.workspaces.is_empty() {
        return Err(IterfileError::MissingSection("workspace"));
    }
    if iterfile.agents.is_empty() {
        return Err(IterfileError::MissingSection("agent"));
    }

    let workspace_decl = iterfile
        .workspaces
        .iter()
        .find(|w| w.node.name == runner.node.workspace)
        .map(|w| &w.node.decl)
        .expect("semantic analyzer validated workspace reference");
    let (workspace_decl, active_source) =
        crate::source::provision_for_workspace(workspace_decl, &iterfile.sources).await?;

    let agent_decl = iterfile
        .agents
        .iter()
        .find(|a| a.node.name == runner.node.agent)
        .map(|a| &a.node.decl)
        .expect("semantic analyzer validated agent reference");
    let prompts = start::prompt_defs_from_expr(&runner.node.prompt, &iterfile.prompts);
    let queue = if let Some(ref queue_name) = runner.node.queue {
        let queue_decl = iterfile
            .queues
            .iter()
            .find(|q| q.node.name == *queue_name)
            .map(|q| &q.node.decl)
            .expect("semantic analyzer validated queue reference");
        Some(crate::queue::queue_from_def(queue_decl)?)
    } else {
        None
    };

    let builder = start::runner_builder_from_plan(
        queue,
        &workspace_decl,
        agent_decl,
        &runner.node,
        &prompts,
        &runner.node.events,
        once,
    )?;

    Ok((builder, iterfile_path.to_owned(), active_source))
}

async fn dispose_active_source(
    active_source: Option<ActiveSource>,
    runtime: Option<&(ProcessRuntime, TerminationRecorder)>,
) -> Result<(), IterfileError> {
    let Some(active_source) = active_source else {
        return Ok(());
    };
    let pending = active_source.dispose().await?;
    let Some(pending) = pending else {
        return Ok(());
    };
    let Some((runtime, _)) = runtime else {
        return Err(SourceBuildError::NoProcessRecord.into());
    };
    let paths = runtime.session().paths();
    crate::source::write_pending_source(paths.dir(), &pending)?;
    Ok(())
}

/// Build a [`RunnerBuilder`](iter_core::RunnerBuilder)-shaped value from
/// a single named service in a `compose.iter` file.
///
/// Used by `iter run --service NAME -f compose.iter`. The compose
/// orchestrator spawns this command for every service whose queue is
/// URL-addressable (file:// or redis://) so that each service appears
/// in `iter ps` / `iter logs` as its own process. The child re-parses
/// compose.iter to build only its own service; sibling services and
/// triggers are not constructed.
///
/// Returns the builder along with the path that the caller will record
/// into the per-process registry entry — the build target's iterfile
/// for `service { build = "./Iterfile" }` services and the compose
/// file itself for inline services.
fn build_compose_service_builder(
    compose_path: &Path,
    service_name: &str,
    once: bool,
) -> Result<(iter_core::RunnerBuilder, PathBuf), IterfileError> {
    let root = load_compose(compose_path).map_err(|e| IterfileError::Compose(Box::new(e)))?;
    let built =
        build_single_service(&root, compose_path, service_name, once).map_err(|e| match e {
            ComposeError::UnknownService(name) => IterfileError::UnknownService {
                path: compose_path.to_owned(),
                name,
            },
            other => IterfileError::Compose(Box::new(other)),
        })?;
    Ok((built.builder, built.iterfile_path))
}

fn load_and_parse(path: &Path) -> Result<Iterfile, IterfileError> {
    let source = std::fs::read_to_string(path).map_err(|source| IterfileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse(&source).map_err(|diags| render_diagnostics(path, &source, &diags))
}

fn render_diagnostics(path: &Path, source: &str, diags: &[Diagnostic]) -> IterfileError {
    if diags.is_empty() {
        return IterfileError::Parse {
            rendered: format!(
                "iterfile {} failed to parse with no diagnostics",
                path.display()
            ),
        };
    }
    let label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Iterfile");
    let mut rendered = String::new();
    for diag in diags {
        rendered.push_str(&diag.report(label, source));
        rendered.push('\n');
    }
    IterfileError::Parse {
        rendered: rendered.trim_end().to_owned(),
    }
}

async fn bootstrap_adopted(
    process_id: ProcessId,
) -> Result<(ProcessRuntime, TerminationRecorder), IterfileError> {
    process_lifecycle::bootstrap_adopted(process_id)
        .await
        .map_err(|err| match err {
            AdoptedBootstrapError::RegistryOpen(source) => IterfileError::RegistryOpen(source),
            AdoptedBootstrapError::Adopt { id, source } => IterfileError::Adopt { id, source },
            AdoptedBootstrapError::Lifecycle(source) => IterfileError::Lifecycle(source),
        })
}
