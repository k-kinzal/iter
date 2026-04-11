//! `RunnerLifecycle` — the system-contract projection that
//! [`crate::process::observer::RunnerObserver`] implementations consume.
//!
//! Per rev17 §F1/§F2, the Runner emits two parallel streams: the
//! user-facing [`Event`](crate::runner::Event) (consumed by
//! [`EventHandler`](crate::runner::EventHandler) hooks defined in iterfile)
//! and the internal `RunnerLifecycle` (consumed by Process-side observers
//! that re-emit each event as a `tracing::info!` record under the
//! `iter::lifecycle` target — fanned into `log.ndjson` by the runtime
//! tracing subscriber, drive `iter ps`, etc.). Lifecycle is the
//! *thinner* of the two: it carries [`SignalId`]s, paths, and timestamps —
//! never prompt bodies, agent output, or full signal payloads. User-defined
//! signal metadata is filtered through [`RedactedMetadata`], which today
//! exposes nothing (allowlist is empty by default).
//!
//! Lifecycle never carries the work the user requested; it carries the
//! shape of what the runner is doing.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentError, AgentReport, ExitStatus};
use crate::runner::ErrorStage;
use crate::signal::{Metadata, SignalId};

/// A single event in the Runner→Process lifecycle stream.
///
/// Variants are arranged in the same order they fire during a normal
/// per-signal turn:
/// `BootstrapStarted` (once at runner start) →
/// `SignalReceived` → `WorkspaceSetup` → `AgentStarting` → `AgentFinished`
/// → `WorkspaceTearDown` (per turn). `BootstrapFailed` and `RunnerError`
/// are out-of-band failure projections.
///
/// `AgentStarting` and `AgentFinished` mirror their
/// [`Event`](crate::runner::Event) counterparts: `AgentStarting` is a
/// pre-step event emitted just before the agent process is launched;
/// `AgentFinished` is a post-step event emitted once the agent has
/// returned (successfully or otherwise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunnerLifecycle {
    /// Runner has begun its initialisation phase. Emitted once, before the
    /// first signal is dequeued, so observers can mark `Initializing` →
    /// "bootstrap visible" in `iter ps`.
    BootstrapStarted {
        /// Wall-clock instant the runner began initialising.
        started_at: DateTime<Utc>,
    },
    /// Runner failed during initialisation (before the loop entered its
    /// steady state).
    BootstrapFailed {
        /// Stringified error from the failing initialisation step.
        error: String,
    },
    /// A [`Signal`](crate::signal::Signal) was successfully dequeued.
    SignalReceived {
        /// Identifier of the dequeued signal.
        signal_id: SignalId,
        /// Allowlisted projection of the signal's metadata.
        metadata: RedactedMetadata,
        /// Wall-clock instant the runner observed the signal.
        ts: DateTime<Utc>,
    },
    /// The workspace finished setup for the named signal.
    WorkspaceSetup {
        /// Identifier of the signal currently being handled.
        signal_id: SignalId,
        /// Filesystem path of the prepared workspace.
        path: PathBuf,
    },
    /// The agent is about to be launched for the named signal. Emitted
    /// immediately before [`Agent::run`](crate::agent::Agent::run);
    /// observers that need a "running visible" cue should treat this as
    /// the rising edge.
    AgentStarting {
        /// Identifier of the signal currently being handled.
        signal_id: SignalId,
    },
    /// The agent run finished (successfully or not) for the named signal.
    AgentFinished {
        /// Identifier of the signal currently being handled.
        signal_id: SignalId,
        /// Coarse-grained outcome category.
        outcome: AgentOutcomeKind,
        /// Process exit code, when one is available.
        exit: Option<i32>,
    },
    /// The workspace finished tearing down for the named signal.
    WorkspaceTearDown {
        /// Identifier of the signal currently being handled.
        signal_id: SignalId,
    },
    /// A runner-level error was observed.
    RunnerError {
        /// Identifier of the signal in flight, when one is in flight.
        signal_id: Option<SignalId>,
        /// Which runner step produced the error.
        stage: ErrorStage,
        /// Stringified error message.
        error_message: String,
    },
}

impl RunnerLifecycle {
    /// Return the [`SignalId`] associated with this lifecycle event, when
    /// one is associated.
    #[must_use]
    pub fn signal_id(&self) -> Option<SignalId> {
        match self {
            Self::BootstrapStarted { .. } | Self::BootstrapFailed { .. } => None,
            Self::SignalReceived { signal_id, .. }
            | Self::WorkspaceSetup { signal_id, .. }
            | Self::AgentStarting { signal_id }
            | Self::AgentFinished { signal_id, .. }
            | Self::WorkspaceTearDown { signal_id } => Some(*signal_id),
            Self::RunnerError { signal_id, .. } => *signal_id,
        }
    }
}

/// Coarse-grained classification of an agent run's outcome.
///
/// Per rev17 §F2, the lifecycle stream does not carry agent stdout, stderr,
/// or report bodies — only this kind plus the optional exit code. This
/// keeps the lifecycle tracing channel bounded in size and free of user
/// payload (agent stdio rides the `log.ndjson` stream directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcomeKind {
    /// Agent process exited 0.
    Success,
    /// Agent process exited with a non-zero code.
    Failure,
    /// Agent process was terminated by a signal.
    TerminatedBySignal,
    /// Platform did not expose either an exit code or a terminating signal.
    UnknownExit,
    /// Agent run was stopped via cancellation. Covers both external
    /// cancellation (the runner's [`CancellationToken`] was fired by the
    /// caller) and internal cancellation (the runner fired an iter-scoped
    /// token because the iteration exceeded its configured timeout).
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    Cancelled,
    /// Agent run failed before producing a report (I/O error, missing
    /// command, hook setup failure, etc.).
    Errored,
}

impl AgentOutcomeKind {
    /// Project a successful [`AgentReport`] into a coarse outcome kind.
    #[must_use]
    pub fn from_report(report: &AgentReport) -> Self {
        match report.exit_status {
            ExitStatus::Success => Self::Success,
            ExitStatus::Failure(_) => Self::Failure,
            ExitStatus::Signal(_) => Self::TerminatedBySignal,
            ExitStatus::Unknown => Self::UnknownExit,
        }
    }

    /// Project an [`AgentError`] into a coarse outcome kind.
    #[must_use]
    pub fn from_error(err: &AgentError) -> Self {
        match err {
            AgentError::Cancelled | AgentError::IterationTimeout(_) => Self::Cancelled,
            AgentError::UnknownExit => Self::UnknownExit,
            AgentError::Io(_)
            | AgentError::EmptyCommand
            | AgentError::HookSetup(_)
            | AgentError::HookStateParse(_) => Self::Errored,
        }
    }

    /// Project a `Result<&AgentReport, &AgentError>` into a coarse outcome
    /// kind. Convenience shortcut around [`Self::from_report`] /
    /// [`Self::from_error`].
    #[must_use]
    pub fn from_result(result: Result<&AgentReport, &AgentError>) -> Self {
        match result {
            Ok(rep) => Self::from_report(rep),
            Err(err) => Self::from_error(err),
        }
    }
}

/// Allowlisted projection of [`Metadata`] for inclusion in the lifecycle
/// stream.
///
/// User-defined metadata can carry arbitrary information (paths,
/// credentials, free text). To keep the lifecycle tracing channel and
/// downstream observers safe to share, this type only carries keys that
/// are explicitly on `RedactedMetadata::ALLOWED_KEYS`. Today that list
/// is empty; keys are added as integration testing demands them, with a
/// review at each addition.
///
/// Iteration order matches `BTreeMap`'s, which is stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedMetadata(BTreeMap<String, String>);

impl RedactedMetadata {
    /// Keys that are permitted to flow into the lifecycle stream.
    ///
    /// Empty by default per rev17 §F2 (advisor: "leave it as
    /// `BTreeMap<String, String>` with empty allowlist for now and add
    /// keys as the integration tests demand them"). Each entry must be a
    /// short, schema-stable identifier — never raw user text or
    /// arbitrary template variables.
    pub const ALLOWED_KEYS: &'static [&'static str] = &[];

    /// An empty `RedactedMetadata`. Equivalent to `Default::default()`.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a `RedactedMetadata` by filtering [`Metadata`] through
    /// [`Self::ALLOWED_KEYS`].
    ///
    /// Values are rendered through their [`Display`](std::fmt::Display)
    /// impl. Non-stringy values (integers, booleans, …) are still safe to
    /// surface because the allowlist itself is the gating mechanism: a key
    /// only flows if it has been explicitly approved.
    #[must_use]
    pub fn from_signal(metadata: &Metadata) -> Self {
        let mut redacted = BTreeMap::new();
        for allowed in Self::ALLOWED_KEYS {
            if let Some(value) = metadata.get_str(allowed) {
                redacted.insert((*allowed).to_owned(), value.to_string());
            }
        }
        Self(redacted)
    }

    /// Borrow the underlying allowlisted entries.
    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Number of allowlisted entries that survived the filter.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when no allowlisted entries are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::metadata::{MetadataKey, MetadataValue};

    fn signal_id() -> SignalId {
        SignalId::new()
    }

    #[test]
    fn signal_id_returns_none_for_bootstrap_variants() {
        let started = RunnerLifecycle::BootstrapStarted {
            started_at: Utc::now(),
        };
        let failed = RunnerLifecycle::BootstrapFailed {
            error: "boom".into(),
        };
        assert_eq!(started.signal_id(), None);
        assert_eq!(failed.signal_id(), None);
    }

    #[test]
    fn signal_id_returns_some_for_per_signal_variants() {
        let id = signal_id();
        let cases = [
            RunnerLifecycle::SignalReceived {
                signal_id: id,
                metadata: RedactedMetadata::empty(),
                ts: Utc::now(),
            },
            RunnerLifecycle::WorkspaceSetup {
                signal_id: id,
                path: PathBuf::from("/tmp/ws"),
            },
            RunnerLifecycle::AgentStarting { signal_id: id },
            RunnerLifecycle::AgentFinished {
                signal_id: id,
                outcome: AgentOutcomeKind::Success,
                exit: Some(0),
            },
            RunnerLifecycle::WorkspaceTearDown { signal_id: id },
        ];
        for ev in cases {
            assert_eq!(ev.signal_id(), Some(id));
        }
    }

    #[test]
    fn signal_id_passthrough_for_runner_error() {
        let id = signal_id();
        let with = RunnerLifecycle::RunnerError {
            signal_id: Some(id),
            stage: ErrorStage::AgentRun,
            error_message: "x".into(),
        };
        let without = RunnerLifecycle::RunnerError {
            signal_id: None,
            stage: ErrorStage::Dequeue,
            error_message: "y".into(),
        };
        assert_eq!(with.signal_id(), Some(id));
        assert_eq!(without.signal_id(), None);
    }

    #[test]
    fn agent_outcome_kind_maps_each_exit_status() {
        let mk = |status| AgentReport {
            exit_status: status,
            last_output: None,
            turn_count: None,
        };
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Success)),
            AgentOutcomeKind::Success
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Failure(2))),
            AgentOutcomeKind::Failure
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Signal(9))),
            AgentOutcomeKind::TerminatedBySignal
        );
        assert_eq!(
            AgentOutcomeKind::from_report(&mk(ExitStatus::Unknown)),
            AgentOutcomeKind::UnknownExit
        );
    }

    #[test]
    fn agent_outcome_kind_maps_each_error() {
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::Cancelled),
            AgentOutcomeKind::Cancelled
        );
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::UnknownExit),
            AgentOutcomeKind::UnknownExit
        );
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::EmptyCommand),
            AgentOutcomeKind::Errored
        );
        let io_err = std::io::Error::other("eio");
        assert_eq!(
            AgentOutcomeKind::from_error(&AgentError::Io(io_err)),
            AgentOutcomeKind::Errored
        );
    }

    #[test]
    fn redacted_metadata_filters_through_empty_allowlist() {
        let mut meta = Metadata::new();
        meta.insert(
            MetadataKey::new("user").unwrap(),
            MetadataValue::String("alice".into()),
        );
        meta.insert(
            MetadataKey::new("path").unwrap(),
            MetadataValue::String("/secret".into()),
        );
        let redacted = RedactedMetadata::from_signal(&meta);
        // Allowlist is empty, so every user-supplied key is dropped.
        assert!(redacted.is_empty());
        assert_eq!(redacted.len(), 0);
        assert!(redacted.as_map().is_empty());
    }

    #[test]
    fn redacted_metadata_default_and_empty_match() {
        assert_eq!(RedactedMetadata::empty(), RedactedMetadata::default());
    }
}
