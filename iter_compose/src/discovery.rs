//! Discover compose-managed runners by scanning `~/.iter/proc/` for the
//! `iter.compose.*` labels stamped at spawn time.
//!
//! `iter compose` is **stateless** — there is no registry record for the
//! orchestrator itself, and no per-project state file. The only ground
//! truth is the labels each child runner persists into its own
//! `meta.json` (`iter.compose.project`, `iter.compose.service`,
//! `iter.compose.orchestrator_pid`, `iter.compose.orchestrator_start_time`).
//!
//! This module reconstructs project state from those labels, mirroring
//! how `docker compose ps` reconstructs project state from container
//! labels alone.
//!
//! # Trust boundary
//!
//! Labels live inside `~/.iter/proc/<id>/meta.json`, which is owned by the
//! invoking user. The discovery layer trusts that any process able to
//! write to that directory is the user themselves (or a process they
//! launched). A same-UID attacker who can plant labels can therefore make
//! [`find_active_orchestrator`] return a fingerprint pointing at an
//! arbitrary same-UID pid; `compose down` would then signal that pid
//! after re-verifying its start-time fingerprint via
//! [`iter_core::process::signal_identity`]. This is the same trust model
//! as `docker compose down` reading container labels — the user-local
//! filesystem is the boundary.
//!
//! It is the discovery primitive shared by:
//!
//! * `compose up -d` — refusing to start a second orchestrator for a
//!   project whose previous orchestrator is still alive
//!   ([`find_active_orchestrator`]).
//! * `compose ls` / `compose ps` — enumerating projects and runners
//!   ([`list_project_members`]).
//! * `compose down` — locating the orchestrator pid to signal
//!   ([`find_active_orchestrator`]).

use std::collections::BTreeMap;
use std::io;

use chrono::{DateTime, Utc};
use iter_core::process::{
    Pid, ProcessError, ProcessIdentity, ProcessRecord, ProcessRegistry, ProcessStartTime,
    ProcessStatus, list_default, process_is_alive_with_start_time,
};
use thiserror::Error;

use crate::compose::{
    LABEL_ORCHESTRATOR_BOOT_ID, LABEL_ORCHESTRATOR_PID, LABEL_ORCHESTRATOR_START_TIME,
    LABEL_PROJECT, LABEL_SERVICE,
};

/// One compose-managed runner, materialised from a registry record.
#[derive(Debug, Clone)]
pub struct ProjectMember {
    /// The runner's registry record.
    pub record: ProcessRecord,
    /// Compose project slug (`iter.compose.project`).
    ///
    /// Carried alongside `service` so callers (e.g. `compose ls`) can group
    /// members by project from a *single* `meta.json` read. Re-reading the
    /// metadata to recover the project label opens a TOCTOU window in
    /// which a concurrent `iter rm` between the two reads would silently
    /// drop the member from the listing — see Codex iter-3 Minor 2.
    pub project: String,
    /// Compose service name (`iter.compose.service`).
    pub service: String,
    /// Runner status as recorded in the status token.
    pub status: ProcessStatus,
    /// When the runner was registered (`meta.json`'s `started_at`).
    ///
    /// Pulled from the same `meta.json` read that built the rest of the
    /// member so `compose ps` does not have to re-open the metadata file
    /// — see Codex iter-3 Minor 3. A second read would race a concurrent
    /// `iter rm` and turn a listing command into a hard `ENOENT` error.
    pub started_at: DateTime<Utc>,
    /// Orchestrator identity stamped into the runner labels.
    pub orchestrator: ProcessIdentity,
}

/// Information needed to act on a still-live orchestrator: signal it
/// (`compose down`) or refuse to start another one (`compose up -d`).
#[derive(Debug, Clone)]
pub struct ActiveOrchestrator {
    /// The compose project this orchestrator owns.
    pub project: String,
    /// pid + start-time fingerprint (`kill -0` + start-time cross-check).
    pub identity: ProcessIdentity,
}

/// Errors returned by the discovery functions.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// Listing process records or reading their metadata failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
}

/// Open the default registry. Used by callers who only need a stable
/// handle for the duration of a single discovery call.
///
/// # Errors
///
/// Returns [`DiscoveryError::Process`] if the registry root cannot be opened.
pub fn open_default_registry() -> Result<ProcessRegistry, DiscoveryError> {
    Ok(ProcessRegistry::open_default()?)
}

/// Enumerate every runner in the local registry that carries
/// `iter.compose.project = <slug>`. Includes terminal records as well as
/// running ones — callers filter on `status` if they care.
///
/// # Errors
///
/// Returns [`DiscoveryError::Process`] if the registry scan or any
/// `meta.json` read fails.
pub fn list_project_members(slug: &str) -> Result<Vec<ProjectMember>, DiscoveryError> {
    let mut out = Vec::new();
    for record in list_default()? {
        let Some(member) = project_member_from_record(&record, Some(slug))? else {
            continue;
        };
        out.push(member);
    }
    Ok(out)
}

/// Group every compose-tagged runner by project. Used by `compose ls`.
///
/// # Errors
///
/// Returns [`DiscoveryError::Process`] if the registry scan fails.
pub fn list_all_members_by_project() -> Result<BTreeMap<String, Vec<ProjectMember>>, DiscoveryError>
{
    let mut by_project: BTreeMap<String, Vec<ProjectMember>> = BTreeMap::new();
    for record in list_default()? {
        let Some(member) = project_member_from_record(&record, None)? else {
            continue;
        };
        // `member.project` was extracted from the same `meta.json` read
        // that built the rest of the member, so a concurrent `iter rm`
        // between this call and the next cannot make `compose ls` and
        // `compose ps` disagree on whether the runner exists.
        by_project
            .entry(member.project.clone())
            .or_default()
            .push(member);
    }
    Ok(by_project)
}

/// Locate the orchestrator that owns `slug`, if one is still alive.
///
/// Walks the registry, finds runners labelled with the project, and for
/// each candidate verifies the orchestrator's pid + start-time
/// fingerprint via [`process_is_alive_with_start_time`]. Returns the
/// first live match. Records whose runner status is terminal still
/// participate — what matters is the orchestrator's liveness, not the
/// runner's, since the orchestrator may be restarting individual
/// services.
///
/// # Errors
///
/// Returns [`DiscoveryError::Process`] if the registry scan or alive
/// check fails.
pub fn find_active_orchestrator(slug: &str) -> Result<Option<ActiveOrchestrator>, DiscoveryError> {
    for member in list_project_members(slug)? {
        if process_is_alive_with_start_time(&member.orchestrator)? {
            return Ok(Some(ActiveOrchestrator {
                project: slug.to_owned(),
                identity: member.orchestrator,
            }));
        }
    }
    Ok(None)
}

fn project_member_from_record(
    record: &ProcessRecord,
    expect_project: Option<&str>,
) -> Result<Option<ProjectMember>, DiscoveryError> {
    // `meta.json` is only readable once `ProcessSession::create_initial`
    // finishes its three-step bootstrap (mkdir → status file → write
    // metadata) AND only stays readable until `cleanup_half_init` /
    // `iter rm` removes the directory. A walker that races either edge
    // sees the directory but `std::fs::read("meta.json")` fails with
    // `ENOENT`. The right semantic for discovery is "this record is in
    // transition; pretend it isn't there yet" — *not* a hard error that
    // aborts `compose ls / ps / down` for every project on the host.
    let Some(meta) = read_metadata_or_skip(record)? else {
        return Ok(None);
    };

    let Some(project) = meta.labels.get(LABEL_PROJECT).cloned() else {
        return Ok(None);
    };
    if let Some(expected) = expect_project
        && project != expected
    {
        return Ok(None);
    }
    let service = meta.labels.get(LABEL_SERVICE).cloned().unwrap_or_default();
    let Some(pid_raw) = meta.labels.get(LABEL_ORCHESTRATOR_PID).cloned() else {
        return Ok(None);
    };
    let Some(start_raw) = meta.labels.get(LABEL_ORCHESTRATOR_START_TIME).cloned() else {
        return Ok(None);
    };
    let pid = pid_raw.parse::<u32>().ok().map(Pid::new).ok_or_else(|| {
        DiscoveryError::Process(ProcessError::CorruptPidFile {
            raw_bytes: pid_raw.into_bytes(),
            reason: format!("{LABEL_ORCHESTRATOR_PID} label not parseable as u32"),
        })
    })?;
    let start_time = ProcessStartTime::from_label_string(&start_raw).map_err(|e| {
        DiscoveryError::Process(ProcessError::CorruptPidFile {
            raw_bytes: start_raw.into_bytes(),
            reason: format!("{LABEL_ORCHESTRATOR_START_TIME} label: {e}"),
        })
    })?;
    // On Linux the orchestrator stamps its own `boot_id` so
    // `process_is_alive_with_start_time` can reject the (otherwise rare)
    // case where pid + tick-since-boot collide across reboots. On macOS
    // the label is intentionally absent — the kernel start-time is
    // already reuse-proof on its own.
    let linux_boot_id = meta.labels.get(LABEL_ORCHESTRATOR_BOOT_ID).cloned();
    let orchestrator = ProcessIdentity {
        pid,
        start_time,
        linux_boot_id,
    };
    // Likewise the status file is written after the directory exists but
    // before the runner is fully published, and it can disappear under
    // `iter rm`. Skip records whose status file isn't readable yet
    // instead of aborting the whole listing.
    let Some(status) = read_status_or_skip(record)? else {
        return Ok(None);
    };
    Ok(Some(ProjectMember {
        record: record.clone(),
        project,
        service,
        status,
        started_at: meta.started_at,
        orchestrator,
    }))
}

/// Read `meta.json`, treating `ENOENT` as "not yet visible / already gone"
/// rather than a hard error. See [`project_member_from_record`].
fn read_metadata_or_skip(
    record: &ProcessRecord,
) -> Result<Option<iter_core::process::ProcessMetadata>, DiscoveryError> {
    match record.metadata() {
        Ok(meta) => Ok(Some(meta)),
        Err(ProcessError::Io(ref e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DiscoveryError::Process(e)),
    }
}

/// Read the status token, treating mid-publication races as "not yet
/// visible / already gone" rather than a hard error. See
/// [`project_member_from_record`].
///
/// Two narrow benign cases collapse to `Ok(None)`:
///
/// * `ENOENT` — the directory exists but the `status` file hasn't been
///   created yet, or `iter rm` removed it between our `list_default()`
///   walk and this read.
/// * The transient empty-body window — `status_file::body::write_status_in_place`
///   does `set_len(0) → write_all` as two syscalls under `flock(LOCK_EX)`,
///   while [`ProcessRecord::read_status_token_or_in_transition`] is
///   unflocked. A reader that lands between the two sees zero bytes;
///   the helper signals this with `Ok(None)` rather than an error so we
///   can distinguish it from genuine corruption (non-UTF8 bytes,
///   unrecognised tokens). Real corruption still propagates as
///   `Err(InvalidData)` so listings don't silently lose broken records
///   from view.
fn read_status_or_skip(record: &ProcessRecord) -> Result<Option<ProcessStatus>, DiscoveryError> {
    match record.read_status_token_or_in_transition() {
        Ok(maybe_status) => Ok(maybe_status),
        Err(ProcessError::Io(ref e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DiscoveryError::Process(e)),
    }
}
