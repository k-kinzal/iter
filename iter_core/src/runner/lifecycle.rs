//! `RunnerLifecycle` — the system-facing lifecycle stream produced by
//! the [`Runner`](crate::runner::Runner).
//!
//! The Runner emits two parallel output streams:
//!
//! - [`Event`](crate::runner::Event): user-facing, rich, hook-oriented.
//!   Consumed by [`EventHandler`](crate::runner::EventHandler)
//!   implementations wired through iterfile `on …` hooks. Carries full
//!   [`Signal`](crate::signal::Signal) payloads, workspace paths, rendered
//!   prompts, and agent reports.
//!
//! - `RunnerLifecycle` (this module): system-facing, slim,
//!   status/log-oriented. Consumed by
//!   [`RunnerObserver`](crate::runner::RunnerObserver) implementations
//!   (the canonical one being
//!   [`LifecycleObserver`](crate::process::observer::LifecycleObserver),
//!   which re-emits each record as `tracing::info!` under
//!   `iter::lifecycle`).
//!
//! Neither stream is a projection of the other. Shared fields may be
//! assembled from the same source values inside the runner, but the two
//! streams are free to evolve independently. Adding a user-only hook
//! event does not require a placeholder variant in `RunnerLifecycle`,
//! and adding a system-only bootstrap record does not require a variant
//! in `Event`.
//!
//! `RunnerLifecycle` carries [`SignalId`]s, paths, and timestamps —
//! never prompt bodies, agent output, or full signal payloads.
//! User-defined signal metadata is filtered through
//! [`RedactedMetadata`], which today exposes nothing (allowlist is
//! empty by default).

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::signal::{Metadata, SignalId};

/// A single event in the Runner's system-facing lifecycle stream.
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
        /// Short result label derived from the agent `Result`: `"success"`
        /// on a clean turn, otherwise the failure class
        /// ([`AgentError::label`](crate::agent::AgentError::label) — e.g.
        /// `"failure"`, `"token_limit"`, `"cancelled"`,
        /// `"terminated_by_signal"`).
        result: String,
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
        /// Which runner step produced the error (e.g. `"dequeue"`,
        /// `"workspace_setup"`, `"agent_run"`, `"workspace_teardown"`,
        /// `"render_prompt"`). Serialized as `"stage"` for backward
        /// compatibility with existing log consumers.
        #[serde(rename = "stage")]
        error_source: String,
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
    /// Empty by default. Each entry must be a short, schema-stable
    /// identifier — never raw user text or arbitrary template variables.
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
                result: "success".to_owned(),
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
            error_source: "agent_run".into(),
            error_message: "x".into(),
        };
        let without = RunnerLifecycle::RunnerError {
            signal_id: None,
            error_source: "dequeue".into(),
            error_message: "y".into(),
        };
        assert_eq!(with.signal_id(), Some(id));
        assert_eq!(without.signal_id(), None);
    }

    #[test]
    fn runner_error_serializes_error_source_as_stage_key() {
        let id = signal_id();
        let lifecycle = RunnerLifecycle::RunnerError {
            signal_id: Some(id),
            error_source: "agent_run".into(),
            error_message: "boom".into(),
        };
        let json = serde_json::to_value(&lifecycle).expect("serialize");
        assert_eq!(json["stage"], "agent_run");
        assert!(
            json.get("error_source").is_none(),
            "field must serialize as 'stage', not 'error_source'"
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
        assert!(redacted.is_empty());
        assert_eq!(redacted.len(), 0);
        assert!(redacted.as_map().is_empty());
    }

    #[test]
    fn redacted_metadata_default_and_empty_match() {
        assert_eq!(RedactedMetadata::empty(), RedactedMetadata::default());
    }

    /// Compile-shape guard: this test has no assertions. Its value is
    /// that it fails to compile if either stream's variant fields change
    /// in a way that breaks the structural independence documented in the
    /// module header.
    #[test]
    fn event_and_lifecycle_evolve_independently() {
        use crate::runner::Event;

        let id = signal_id();

        // User-only events: no RunnerLifecycle counterpart required.
        drop(Event::RunnerStarting {});
        drop(Event::RunnerFinished {
            reason: crate::runner::RunnerTerminationReason::Cancelled,
            iteration_count: 0,
        });

        // System-only lifecycle records: no Event counterpart required.
        drop(RunnerLifecycle::BootstrapStarted {
            started_at: Utc::now(),
        });
        drop(RunnerLifecycle::BootstrapFailed { error: "x".into() });

        // Shared concept — both streams carry it in their own shape.
        drop(Event::AgentFinished {
            signal: crate::signal::Signal::synthesized(),
            path: PathBuf::from("/tmp"),
            result: Ok(crate::agent::AgentRun::empty()),
        });
        drop(RunnerLifecycle::AgentFinished {
            signal_id: id,
            result: "success".to_owned(),
            exit: Some(0),
        });
    }
}
