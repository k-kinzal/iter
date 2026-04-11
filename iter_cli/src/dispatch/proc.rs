//! Handlers for the process-management subcommands:
//! `ps` (alias for `process ls`), `logs`, `stop`, `kill`, `rm`, `inspect`.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use iter_core::process::{
    LogStream, PidFileState, ProcessError, ProcessHandle, ProcessId, ProcessRecord,
    ProcessRegistry, ProcessStatus, process_is_alive_with_start_time,
};
use serde::Serialize;
use std::io::Write;
use thiserror::Error;

use crate::cli::{InspectArgs, LogsArgs, PsArgs, TargetArgs};
use crate::output::{
    IntoExitCode, OutputFormat, Table, cli_eprintln, cli_println, exit_codes, print_json_pretty,
    print_ndjson_record, relative_time, trunc_id,
};

/// Errors produced by the `iter ps`, `logs`, `stop`, `kill`, `rm`, and
/// `inspect` subcommands.
#[derive(Debug, Error)]
pub enum ProcessCmdError {
    /// Underlying process-registry / handle / record error.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// Reading log lines from disk failed.
    #[error("reading log line: {0}")]
    LogIo(#[source] ProcessError),
    /// `iter inspect` failed to serialize the metadata to JSON.
    #[error("serializing metadata: {0}")]
    JsonSerialize(#[source] serde_json::Error),
    /// Process lookup turned up no records.
    #[error("process {0} not found")]
    NotFound(String),
    /// Process name matches multiple terminal records.
    #[error("process {0} matches multiple terminal records; specify by id")]
    AmbiguousTerminal(String),
    /// Process name matches multiple active records.
    #[error("process {0} is ambiguous (multiple active records); specify by id")]
    AmbiguousActive(String),
    /// ID prefix matches more than one record.
    #[error("process {0} is ambiguous (multiple id matches); specify more characters")]
    AmbiguousPrefix(String),
    /// `iter rm` was invoked against a process whose pid is still alive.
    #[error("process {target} is still running (pid {pid}); wait for exit before `iter rm`")]
    StillRunning {
        /// User-supplied target.
        target: String,
        /// PID of the still-running process.
        pid: u32,
    },
}

impl IntoExitCode for ProcessCmdError {
    fn exit_code(&self) -> i32 {
        match self {
            // Bad target / unknown id / wrong-mode operation → user input.
            Self::NotFound(_)
            | Self::AmbiguousTerminal(_)
            | Self::AmbiguousActive(_)
            | Self::AmbiguousPrefix(_)
            | Self::StillRunning { .. } => exit_codes::USER_INPUT,
            // Disk / registry / log-read failures are runtime issues.
            Self::Process(_) | Self::LogIo(_) => exit_codes::RUNTIME,
            // Serialisation should never happen on a well-formed record.
            Self::JsonSerialize(_) => exit_codes::INTERNAL,
        }
    }
}

/// One row in the `iter ps` table / NDJSON output.
#[derive(Debug, Serialize)]
struct PsRecord {
    id: String,
    name: String,
    status: String,
    pid: Option<u32>,
    created_at: DateTime<Utc>,
    iterfile: PathBuf,
}

/// `iter ps` / `iter process ls` — list every process record managed by
/// the local registry.
///
/// `all = false` hides terminal records (`Stopped` / `Failed` / `Killed`)
/// after a flock-protected reconcile but keeps live records — including
/// `Initializing`, which is exactly what an operator needs to see when
/// diagnosing an adoption or bootstrap that has not yet flipped to
/// `Running`. `all = true` shows every directory regardless of status.
///
/// # Errors
///
/// Returns an error when the registry cannot be opened or the directory
/// listing fails.
pub async fn run_ps(args: PsArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let records = registry.list()?;
    let mut rows: Vec<PsRecord> = Vec::with_capacity(records.len());
    for record in records {
        let id = record.id();
        let status = match ProcessHandle::open(registry.proc_root(), id).await {
            Ok(handle) => match handle.refresh_status().await {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(%id, %err, "ps: refresh_status failed; skipping record");
                    continue;
                }
            },
            Err(err) => {
                tracing::warn!(%id, %err, "ps: open failed; skipping record");
                continue;
            }
        };
        if !args.all && status.is_terminal() {
            continue;
        }
        let name = record.name().unwrap_or_else(|_| "?".to_owned());
        let pid = match record.pid_identity() {
            PidFileState::Found(identity) => Some(identity.pid.as_raw()),
            _ => None,
        };
        // Mid-publication race: `ProcessSession::create_initial` writes
        // the directory + status file *before* `meta.json` (the side-files
        // first, then `meta.json` last so its presence implies every
        // companion is readable — see `iter_core::process::session`). A
        // walker that races between the status write and the metadata
        // write sees `Initializing` here but `metadata()` returns
        // `ENOENT`. The right semantic for a *listing* command is to
        // skip the in-flight record, not to abort the whole `iter ps`.
        // The same skip applies to a record being torn down by `iter rm`
        // between the two reads.
        let metadata = match record.metadata() {
            Ok(m) => m,
            Err(ProcessError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(%id, "ps: meta.json missing (mid-publication or torn down); skipping record");
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        rows.push(PsRecord {
            id: id.to_string(),
            name,
            status: status.as_serde_str().to_owned(),
            pid,
            created_at: metadata.started_at,
            iterfile: metadata.iterfile,
        });
    }

    if args.listing.quiet {
        for row in &rows {
            cli_println!("{}", trunc_id(&row.id, args.listing.no_trunc));
        }
        return Ok(());
    }

    match args.listing.format {
        OutputFormat::Json => {
            for row in &rows {
                print_ndjson_record(row).map_err(ProcessCmdError::JsonSerialize)?;
            }
        }
        OutputFormat::Table => {
            print_ps_table(&rows, args.listing.no_trunc);
        }
    }
    Ok(())
}

fn print_ps_table(rows: &[PsRecord], no_trunc: bool) {
    let mut table = Table::new(&["ID", "NAME", "STATUS", "PID", "CREATED", "ITERFILE"]);
    for row in rows {
        let iterfile_short = row
            .iterfile
            .file_name()
            .and_then(|n| n.to_str())
            .map_or_else(|| row.iterfile.display().to_string(), str::to_owned);
        let created = relative_time(row.created_at);
        table.row([
            trunc_id(&row.id, no_trunc),
            row.name.clone(),
            row.status.clone(),
            row.pid.map_or_else(|| "?".to_owned(), |p| p.to_string()),
            created,
            iterfile_short,
        ]);
    }
    table.print();
}

/// `iter logs <process>` — replay the per-process `log.ndjson`.
///
/// Mirrors `docker logs`: stdout records flow to the CLI's stdout, stderr
/// records to the CLI's stderr, so `iter logs <id> 2>/dev/null` extracts
/// just the agent's intentional output. With `--timestamps`/`-t` each
/// line is prefixed by an RFC3339 microsecond timestamp, mirroring the
/// `docker logs -t` shape.
///
/// `iter run` always wires the runtime's stdio sink to a per-process
/// `log.ndjson`. The only case where there is nothing to show is an
/// interactive TTY agent (passthrough stdio), which never opens the log
/// file in the first place; the reader yields no entries and the call
/// returns `Ok(())`.
///
/// # Errors
///
/// Returns an error when the process cannot be found or its log cannot be
/// opened.
pub async fn run_logs(args: LogsArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let record = lookup(&registry, &args.instance)?;
    let mut reader = record.tail_log_ndjson(args.follow, args.tail)?;
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    while let Some(entry) = reader.next_entry().await.map_err(ProcessCmdError::LogIo)? {
        let line = if args.timestamps {
            format!(
                "{} {}",
                entry
                    .ts
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                entry.line
            )
        } else {
            entry.line.clone()
        };
        match entry.stream {
            LogStream::Stdout => {
                let mut handle = stdout.lock();
                drop(writeln!(handle, "{line}"));
            }
            LogStream::Stderr => {
                let mut handle = stderr.lock();
                drop(writeln!(handle, "{line}"));
            }
        }
    }
    Ok(())
}

/// `iter stop <process>` — send `SIGTERM` and mark the record `Killed`.
///
/// Confirmation goes to **stderr**: stdout is reserved for ID-shaped output
/// that scripts capture. `--quiet` suppresses the confirmation entirely; a
/// successful exit code is the only signal.
///
/// # Errors
///
/// Returns an error when the process cannot be found or signalling fails.
pub async fn run_stop(args: TargetArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let record = lookup(&registry, &args.instance)?;
    let handle = ProcessHandle::open(registry.proc_root(), record.id()).await?;
    if let Some(status) = already_terminal(&handle).await? {
        if !args.quiet {
            cli_eprintln!("{}: already {}", record.id(), status.as_serde_str());
        }
        return Ok(());
    }
    let result = handle.stop().await?;
    if !args.quiet {
        cli_eprintln!("{}: {} -> {}", record.id(), result.from, result.to);
    }
    Ok(())
}

/// `iter kill <process>` — send `SIGKILL` and mark the record `Killed`.
/// # Errors
///
/// Returns an error if the operation fails.
///
/// `iter stop` flips the record to `Killed` synchronously after sending
/// SIGTERM, but the underlying child may still be alive (stuck in a
/// non-cancellable agent or hook). The whole point of `iter kill` is the
/// forceful escalation, so this path probes the recorded pid before
/// short-circuiting:
///
/// - non-terminal record → `handle.kill()` (signals + transitions)
/// - terminal record + pid still alive → `handle.force_kill()` (signals
///   only, no transition)
/// - terminal record + pid gone → no-op `already <state>` message
pub async fn run_kill(args: TargetArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let record = lookup(&registry, &args.instance)?;
    let handle = ProcessHandle::open(registry.proc_root(), record.id()).await?;
    if let Some(status) = already_terminal(&handle).await? {
        let escalated = handle.force_kill()?;
        if !args.quiet {
            if escalated {
                cli_eprintln!(
                    "{}: {} (force-killed live pid)",
                    record.id(),
                    status.as_serde_str()
                );
            } else {
                cli_eprintln!("{}: already {}", record.id(), status.as_serde_str());
            }
        }
        return Ok(());
    }
    let result = handle.kill().await?;
    if !args.quiet {
        cli_eprintln!("{}: {} -> {}", record.id(), result.from, result.to);
    }
    Ok(())
}

/// `iter rm <process>` — remove a terminal process directory.
///
/// `stop`/`kill` flip the on-disk status to `Killed` synchronously, but the
/// underlying child exit is asynchronous. We first call `refresh_status` so
/// records whose runner died outside the finalize path get promoted to a
/// terminal state on this call (otherwise `handle.remove` would refuse a
/// stale `Running`), then re-check the recorded PID via
/// [`process_is_alive_with_start_time`] so a racy `iter stop && iter rm`
/// does not wipe the proc directory while the runner is still writing to
/// its log files.
///
/// # Errors
///
/// Returns an error when the process cannot be found, is still running, or
/// the directory cannot be removed.
pub async fn run_rm(args: TargetArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let record = lookup(&registry, &args.instance)?;
    let handle = ProcessHandle::open(registry.proc_root(), record.id()).await?;
    handle.refresh_status().await?;
    if let PidFileState::Found(identity) = record.pid_identity() {
        // Probe errors must bias toward refusing removal — silently
        // treating them as "dead" would let `iter rm` race with a live
        // runner and delete the proc dir out from under its log writes.
        let alive = process_is_alive_with_start_time(&identity)?;
        if alive {
            return Err(ProcessCmdError::StillRunning {
                target: args.instance,
                pid: identity.pid.as_raw(),
            });
        }
    }
    handle.remove().await?;
    if !args.quiet {
        cli_eprintln!("removed {}", record.id());
    }
    Ok(())
}

/// Reconcile the persisted status. If the record is already terminal (or
/// the live process probe just promoted a stale `Running` to a terminal
/// state), return that status so the caller can short-circuit instead of
/// signalling — which would otherwise risk SIGTERM/SIGKILL hitting a
/// reused PID.
async fn already_terminal(handle: &ProcessHandle) -> Result<Option<ProcessStatus>, ProcessError> {
    let status = handle.refresh_status().await?;
    Ok(status.is_terminal().then_some(status))
}

/// `iter inspect <process>` — print the JSON metadata for a process record.
///
/// Always emits canonical JSON: inspect is the source of truth for a
/// resource (P8). Operators wanting a tabular view should use `iter ps`;
/// hence no `--format` flag.
///
/// # Errors
///
/// Returns an error when the process cannot be found or the metadata file
/// cannot be deserialized.
pub async fn run_inspect(args: InspectArgs) -> Result<(), ProcessCmdError> {
    let registry = open_registry()?;
    let record = lookup(&registry, &args.instance)?;
    let meta = record.metadata()?;
    print_json_pretty(&meta).map_err(ProcessCmdError::JsonSerialize)?;
    Ok(())
}

/// Open the default `~/.iter/proc` registry wrapped in an [`Arc`].
fn open_registry() -> Result<Arc<ProcessRegistry>, ProcessError> {
    let registry = ProcessRegistry::open_default()?;
    Ok(Arc::new(registry))
}

/// Resolve `id_or_name` to a [`ProcessRecord`].
///
/// Three resolution stages, in order:
///
/// 1. Full ULID parse → `registry.get(id)`.
/// 2. Exact name match. After stale-lock recovery multiple records can
///    share a name — the previous run's proc directory remains until
///    `iter rm`. We disambiguate by preferring the single live record,
///    where "live" means *either* a non-terminal status *or* a recorded
///    pid that the kernel still reports running. The latter matters
///    because `iter stop` flips status to `Killed` synchronously while
///    the child may still be executing — without the pid check,
///    `iter kill <name>` immediately after `iter stop <name>` would
///    report "ambiguous" instead of selecting the still-live record.
/// 3. ID prefix match (Docker-style). Both sides are normalized to
///    ASCII lower-case before `starts_with`, so prefixes resolve against
///    historical upper-case ULID directories as well as new lower-case
///    ones. Zero matches → `NotFound`; one match → that record;
///    multiple matches → `AmbiguousPrefix` (no minimum length: shorter
///    prefixes that happen to be unique are accepted).
fn lookup(registry: &ProcessRegistry, id_or_name: &str) -> Result<ProcessRecord, ProcessCmdError> {
    if let Ok(id) = id_or_name.parse::<ProcessId>() {
        return registry.get(id).map_err(ProcessCmdError::Process);
    }
    let records = registry.list()?;
    let name_matches: Vec<ProcessRecord> = records
        .iter()
        .filter(|rec| matches!(rec.name(), Ok(n) if n == id_or_name))
        .cloned()
        .collect();
    if !name_matches.is_empty() {
        return resolve_name_matches(name_matches, id_or_name);
    }
    let needle = id_or_name.to_ascii_lowercase();
    let mut prefix_matches: Vec<ProcessRecord> = records
        .into_iter()
        .filter(|rec| {
            rec.id()
                .to_string()
                .to_ascii_lowercase()
                .starts_with(&needle)
        })
        .collect();
    match prefix_matches.len() {
        0 => Err(ProcessCmdError::NotFound(id_or_name.to_owned())),
        1 => Ok(prefix_matches.pop().unwrap()),
        _ => Err(ProcessCmdError::AmbiguousPrefix(id_or_name.to_owned())),
    }
}

fn resolve_name_matches(
    mut matches: Vec<ProcessRecord>,
    id_or_name: &str,
) -> Result<ProcessRecord, ProcessCmdError> {
    if matches.len() == 1 {
        return Ok(matches.pop().unwrap());
    }
    let mut active = Vec::new();
    for rec in matches {
        if record_is_live(&rec)? {
            active.push(rec);
        }
    }
    match active.len() {
        0 => Err(ProcessCmdError::AmbiguousTerminal(id_or_name.to_owned())),
        1 => Ok(active.pop().unwrap()),
        _ => Err(ProcessCmdError::AmbiguousActive(id_or_name.to_owned())),
    }
}

/// True iff the record is non-terminal *or* its recorded pid is still alive.
///
/// Probe errors are propagated so a transient `/proc` / `proc_pidinfo`
/// failure surfaces as a clear lookup error rather than silently
/// excluding a record that may actually be the user's target.
fn record_is_live(rec: &ProcessRecord) -> Result<bool, ProcessError> {
    if let Ok(status) = rec.read_status_token()
        && !status.is_terminal()
    {
        return Ok(true);
    }
    if let PidFileState::Found(identity) = rec.pid_identity() {
        process_is_alive_with_start_time(&identity)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use iter_core::process::MetadataDraft;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sample_draft() -> MetadataDraft {
        MetadataDraft {
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into()],
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        }
    }

    async fn register(registry: &ProcessRegistry, name: &str) -> ProcessId {
        let (session, lock) = registry
            .register_foreground(name, sample_draft())
            .await
            .expect("register_foreground");
        let id = session.id();
        // Drop guards so the record persists as a "registered" entry the
        // lookup() function can iterate. The directory itself stays put
        // because we do not call cleanup.
        drop(session);
        // Keep the lock body on disk: dropping (rather than releasing)
        // closes the FD and leaves the file as the name registry entry.
        drop(lock);
        id
    }

    #[tokio::test]
    async fn lookup_resolves_unique_id_prefix() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).unwrap();
        let id = register(&registry, "alpha").await;
        let s = id.to_string();
        // 8-char prefix is overwhelmingly likely to be unique with one record.
        let prefix = &s[..8];
        let rec = lookup(&registry, prefix).expect("prefix resolves");
        assert_eq!(rec.id(), id);
    }

    #[tokio::test]
    async fn lookup_ambiguous_prefix_errors_with_marker() {
        // Two ULIDs generated within the same millisecond share their
        // 10-char timestamp prefix; a 1-char "0" prefix is enough to make
        // any pair collide because every ULID begins with `01`.
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).unwrap();
        let _id1 = register(&registry, "one").await;
        let _id2 = register(&registry, "two").await;

        let err = lookup(&registry, "0").expect_err("must be ambiguous");
        assert!(
            matches!(err, ProcessCmdError::AmbiguousPrefix(ref s) if s == "0"),
            "expected AmbiguousPrefix; got {err:?}"
        );
    }

    #[tokio::test]
    async fn lookup_prefix_matches_uppercase_directory_via_lowercase_input() {
        // ProcessId::Display now renders lower-case, but historical
        // directories may already exist on disk in upper-case form. Both
        // sides of the comparison normalize to ASCII lower-case, so the
        // user can paste a lower-case prefix and still hit an upper-case
        // directory from a previous version.
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).unwrap();
        let id = register(&registry, "legacy").await;

        // Forge an upper-case sibling directory by renaming.
        let lower_dir = tmp.path().join(id.to_string());
        let upper = id.to_string().to_ascii_uppercase();
        let upper_dir = tmp.path().join(&upper);
        std::fs::rename(&lower_dir, &upper_dir).expect("rename to upper-case");

        // Use a lower-case prefix; lookup should still resolve.
        let prefix_lc = id.to_string()[..10].to_string();
        let rec = lookup(&registry, &prefix_lc).expect("lower-case prefix resolves");
        assert_eq!(rec.id(), id);
    }

    #[tokio::test]
    async fn lookup_unknown_prefix_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).unwrap();
        let _id = register(&registry, "alpha").await;

        let err = lookup(&registry, "ffffffffff").expect_err("must not match");
        assert!(
            matches!(err, ProcessCmdError::NotFound(ref s) if s == "ffffffffff"),
            "expected NotFound; got {err:?}"
        );
    }
}
