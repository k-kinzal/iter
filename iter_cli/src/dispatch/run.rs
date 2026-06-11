//! Clap-side adapter for `iter run`.
//!
//! This file's only job is to translate the Clap-bound [`RunArgs`] into
//! an [`iter_compose::iterfile::RunInput`] and hand off to
//! [`iter_compose::iterfile::handle`]. All iter-side logic — iterfile
//! reading, parsing, runner construction, process-registry bookkeeping,
//! and finalisation — lives in `iter_compose::iterfile`.
//!
//! Iterfile loading deliberately happens inside the handler, not here:
//! when `--process-id` is set, the parent has already allocated an
//! `Initializing` registry record. Adoption must run before any
//! fallible step (including reading the Iterfile) so a missing or
//! invalid Iterfile still flips the record to a terminal state via the
//! handler's finalize-on-return path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iter_compose::iterfile::{
    self, IterfileError, RunInput, RunMode, RunRecordMetadata, RunSource,
};
use iter_core::process::ProcessId;
use thiserror::Error;

use crate::cli::RunArgs;
use crate::dispatch::load::DEFAULT_ITERFILE;
use crate::naming::default_process_name;
use crate::output::{IntoExitCode, exit_codes};
use crate::telemetry;
use crate::tracing_preferences::TracingPreferences;

/// Errors produced by [`run_run`].
#[derive(Debug, Error)]
pub enum RunCmdError {
    /// `--process-id` could not be parsed as a [`ProcessId`].
    #[error("parsing --process-id {raw}: {detail}")]
    ParseProcessId {
        /// The raw `--process-id` argument.
        raw: String,
        /// Rendered parse error message.
        detail: String,
    },
    /// The user-named iterfile does not exist on disk.
    ///
    /// Mirrors the detach path's `IterCliError::IterfileMissing` so both
    /// surfaces return the same `"iterfile not found at <path>"` message
    /// and `USER_INPUT` exit code; without this, the foreground path
    /// would surface `IterfileError::Canonicalise` with a less friendly
    /// `"No such file or directory (os error 2)"` tail and exit `RUNTIME`
    /// (the contract reserves `RUNTIME` for genuine I/O faults, e.g. the
    /// TOCTOU race that `CanonicaliseIterfile` covers).
    #[error("iterfile not found at {}", path.display())]
    IterfileMissing {
        /// The path the user named.
        path: PathBuf,
    },
    /// Canonicalising the user-named iterfile path failed for a reason
    /// other than "missing" (e.g. permission denied).
    #[error("canonicalising iterfile path {}: {source}", path.display())]
    CanonicaliseIterfile {
        /// The path the user named.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A `--arg` value was malformed (missing `=`).
    #[error("invalid --arg `{raw}`: expected KEY=VALUE format")]
    BadArgOverride {
        /// The malformed value.
        raw: String,
    },
    /// The iterfile run handler returned an error.
    #[error(transparent)]
    Iterfile(#[from] IterfileError),
}

impl IntoExitCode for RunCmdError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::ParseProcessId { .. }
            | Self::IterfileMissing { .. }
            | Self::BadArgOverride { .. } => exit_codes::USER_INPUT,
            // `canonicalize` is reached only after `raw.exists()` returned
            // true at the preflight check, so a failure here means a TOCTOU
            // race or a permission flip — a runtime/I/O fault, not bad
            // user input. Symmetrical with the `IterfileError::Canonicalise`
            // arm in `iterfile_error_exit_code` below.
            Self::CanonicaliseIterfile { .. } => exit_codes::RUNTIME,
            Self::Iterfile(e) => iterfile_error_exit_code(e),
        }
    }
}

/// Map an [`IterfileError`] to a typed exit code.
///
/// Keeps the per-variant policy in one place so a future variant cannot
/// be silently routed through a blanket fallback.
fn iterfile_error_exit_code(e: &IterfileError) -> i32 {
    match e {
        // `run_run` pre-canonicalises and rejects missing paths up front,
        // so reaching `Canonicalise` here means a TOCTOU race or a
        // permission flip after the existence check — RUNTIME, not
        // USER_INPUT.
        IterfileError::Canonicalise { .. }
        | IterfileError::Read { .. }
        | IterfileError::QueueBuild(_)
        | IterfileError::Runner(_)
        | IterfileError::Lifecycle(_) => exit_codes::RUNTIME,
        IterfileError::Assembly(iter_compose::AssemblyError::QueueBuild(_)) => exit_codes::RUNTIME,
        IterfileError::Parse { .. }
        | IterfileError::MissingSection(_)
        | IterfileError::Arg(_)
        | IterfileError::Assembly(_)
        | IterfileError::Builder(_)
        | IterfileError::Compose(_)
        | IterfileError::UnknownService { .. } => exit_codes::CONFIG,
        IterfileError::RegistryOpen(_)
        | IterfileError::Adopt { .. }
        | IterfileError::FinalizeStatus(_) => exit_codes::INTERNAL,
    }
}

/// Handle `iter run`.
///
/// Translates [`RunArgs`] into a [`RunInput`] and forwards to the
/// iter-side handler. The detached (`--process-id <ULID>`) and
/// foreground branches differ only in which [`RunMode`] is set.
///
/// # Errors
///
/// Returns the error surfaced by [`iter_compose::iterfile::handle`].
pub async fn run_run(args: RunArgs, prefs: TracingPreferences) -> Result<(), RunCmdError> {
    let iterfile_path: PathBuf = args
        .iterfile
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ITERFILE));
    // Pre-validate the path so the foreground branch surfaces the same
    // friendly "iterfile not found at X" diagnostic + USER_INPUT exit code
    // that `canonical_iterfile` produces for the detach branch. The
    // adopted (`--process-id`) branch deliberately bypasses this check —
    // the parent has already allocated a registry record that needs to
    // be flipped to a terminal state, and `iter_compose::iterfile::handle`
    // owns that finalisation.
    let iterfile_path = if args.process_id.is_some() {
        iterfile_path
    } else {
        canonical_iterfile_for_run(&iterfile_path)?
    };

    let _telemetry_guard = init_run_telemetry(&args, &prefs, &iterfile_path);

    let mode = if let Some(raw) = args.process_id.as_deref() {
        let process_id = raw
            .parse::<ProcessId>()
            .map_err(|err| RunCmdError::ParseProcessId {
                raw: raw.to_owned(),
                detail: err.to_string(),
            })?;
        RunMode::Adopted { process_id }
    } else {
        let default_name = match &args.service {
            Some(svc) => svc.clone(),
            None => default_process_name(&iterfile_path),
        };
        let name = args.name.clone().unwrap_or(default_name);
        RunMode::Foreground { name }
    };

    let source = match &args.service {
        Some(name) => RunSource::ComposeService {
            service_name: name.clone(),
        },
        None => RunSource::Iterfile,
    };

    let arg_overrides = parse_arg_overrides(&args.arg)?;

    let input = RunInput {
        iterfile_path,
        source,
        once: args.once,
        mode,
        metadata: RunRecordMetadata {
            argv: rebuild_argv(&args),
            subcommand: "run".into(),
            debug: args.debug,
        },
        arg_overrides,
    };

    Box::pin(iterfile::handle(input)).await?;
    Ok(())
}

fn init_run_telemetry(
    args: &RunArgs,
    prefs: &TracingPreferences,
    iterfile_path: &Path,
) -> telemetry::TelemetryGuard {
    let Some(service_name) = args.service.as_deref() else {
        return telemetry::init(args.debug, prefs);
    };
    let Ok(root) = iter_compose::load_compose(iterfile_path) else {
        return telemetry::init(args.debug, prefs);
    };
    let project =
        iter_compose::project_slug(iterfile_path, None).unwrap_or_else(|_| "iter".to_string());
    telemetry::init_for_compose(
        args.debug,
        prefs,
        root.telemetry.as_ref().map(|t| &t.node),
        &project,
        Some(service_name),
    )
}

/// Resolve a user-supplied iterfile path to its canonical form, surfacing a
/// `RunCmdError::IterfileMissing` (`USER_INPUT`) when the path does not exist.
///
/// Mirrors [`crate::main::canonical_iterfile`] for the foreground branch so
/// that "wrong path" errors stay `USER_INPUT` regardless of whether the user
/// passed `--detach`.
fn canonical_iterfile_for_run(raw: &Path) -> Result<PathBuf, RunCmdError> {
    if !raw.exists() {
        return Err(RunCmdError::IterfileMissing {
            path: raw.to_path_buf(),
        });
    }
    raw.canonicalize()
        .map_err(|source| RunCmdError::CanonicaliseIterfile {
            path: raw.to_path_buf(),
            source,
        })
}

fn parse_arg_overrides(raw: &[String]) -> Result<BTreeMap<String, String>, RunCmdError> {
    let mut map = BTreeMap::new();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| RunCmdError::BadArgOverride { raw: entry.clone() })?;
        map.insert(key.to_owned(), value.to_owned());
    }
    Ok(map)
}

fn rebuild_argv(args: &RunArgs) -> Vec<String> {
    let mut out = vec!["run".to_owned()];
    if let Some(p) = args.iterfile.as_ref() {
        out.push(p.display().to_string());
    }
    if let Some(c) = args.config.as_ref() {
        out.push("--config".into());
        out.push(c.display().to_string());
    }
    if args.once {
        out.push("--once".into());
    }
    if args.debug {
        out.push("--debug".into());
    }
    if let Some(name) = args.name.as_ref() {
        out.push("--name".into());
        out.push(name.clone());
    }
    if let Some(svc) = args.service.as_ref() {
        out.push("--service".into());
        out.push(svc.clone());
    }
    for entry in &args.arg {
        out.push("--arg".into());
        out.push(entry.clone());
    }
    out
}
