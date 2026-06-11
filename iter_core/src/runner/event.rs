//! [`Event`] stream produced by the [`Runner`](crate::runner::Runner).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::agent::AgentRun;
use crate::prompt::Prompt;
use crate::runner::config::RunnerTerminationReason;
use crate::signal::{Signal, SignalId};

/// Shared, read-only handle to the [`Signal`] a runner is processing.
///
/// Every per-signal lifecycle [`Event`] emitted during one bracket execution
/// carries the *same* `SharedSignal`, cloned from a single allocation created
/// when the runner enters the bracket. Cloning is an [`Arc`] reference-count
/// bump — not a deep copy of the signal's
/// [`Metadata`](crate::signal::Metadata) map — so emitting seven events for
/// one signal costs seven pointer clones, not seven metadata clones.
///
/// The handle is read-only: a [`Signal`] is immutable once constructed, and
/// `SharedSignal` exposes only borrowed access to it. Treat it as a `&Signal`
/// (it [`Deref`](std::ops::Deref)s to one, and [`Event::signal`] /
/// [`Event::signal_id`] hand back borrowed views); the `Arc` storage is an
/// implementation detail consumers must not depend on.
///
/// The single-allocation sharing is an **in-process** property of one runner's
/// bracket execution. `SharedSignal` serializes transparently as its inner
/// [`Signal`], so a deserialized event carries its own fresh allocation;
/// allocation identity (e.g. pointer equality) must not be relied upon across
/// a serialization boundary — only the signal *value* survives the round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSignal(Arc<Signal>);

impl SharedSignal {
    /// Wrap an owned signal in a shareable, reference-counted handle.
    #[must_use]
    pub fn new(signal: Signal) -> Self {
        Self(Arc::new(signal))
    }

    /// Borrow the underlying signal as a bare `&Signal`.
    ///
    /// `SharedSignal` also [`Deref`](std::ops::Deref)s to [`Signal`], so
    /// `Signal` methods can be called directly on the handle. Reach for this
    /// named accessor when you need the `&Signal` *as a value* — handing it to
    /// a function that requires the bare type (e.g. the runner's
    /// `render_prompt`), or comparing allocation identity with
    /// [`std::ptr::eq`] — where deref coercion does not apply.
    #[must_use]
    pub fn as_signal(&self) -> &Signal {
        &self.0
    }
}

impl std::ops::Deref for SharedSignal {
    type Target = Signal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Signal> for SharedSignal {
    fn from(signal: Signal) -> Self {
        Self::new(signal)
    }
}

// A `SharedSignal` serializes transparently as its inner [`Signal`], so the
// on-the-wire shape of an [`Event`] is unaffected by the shared-handle
// storage (and the derive needs no serde `rc` feature).
impl Serialize for SharedSignal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SharedSignal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Signal::deserialize(deserializer).map(Self::new)
    }
}

/// Event emitted by the [`Runner`](crate::runner::Runner) between every step
/// of the per-signal loop.
///
/// Per-signal lifecycle events carry the signal as a [`SharedSignal`] (not
/// just its id) so that [`EventHandler`](crate::runner::EventHandler)
/// implementations — such as template-rendering shell handlers — have direct
/// access to the signal's metadata without an external lookup table. All
/// events emitted for one bracket share a single signal allocation rather than
/// each owning an independent deep copy; inspect the signal through the
/// borrowed [`Event::signal`] / [`Event::signal_id`] accessors so call sites
/// stay independent of the shared-handle storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The runner is about to enter its per-signal loop. Fired exactly once,
    /// before any signal is dequeued and before any other lifecycle event.
    ///
    /// Has no signal context — handlers wired to `on runner_starting` cannot
    /// reference `{{signal.*}}` template variables.
    RunnerStarting {},
    /// A signal was successfully dequeued.
    SignalReceived {
        /// The dequeued signal.
        signal: SharedSignal,
    },
    /// The runner is about to call [`Workspace::setup`](crate::workspace::Workspace::setup).
    WorkspaceSetupStarting {
        /// Signal currently being handled.
        signal: SharedSignal,
    },
    /// The workspace finished setup.
    WorkspaceSetupFinished {
        /// Signal currently being handled.
        signal: SharedSignal,
        /// Filesystem path of the prepared workspace.
        path: PathBuf,
    },
    /// The runner is about to invoke the agent.
    AgentStarting {
        /// Signal currently being handled.
        signal: SharedSignal,
        /// Workspace path supplied to the agent.
        path: PathBuf,
        /// Rendered prompt supplied to the agent.
        prompt: Prompt,
    },
    /// The agent run completed (successfully or not).
    AgentFinished {
        /// Signal currently being handled.
        signal: SharedSignal,
        /// Workspace path supplied to the agent.
        path: PathBuf,
        /// Result of the agent run, with the error stringified. `Ok` means
        /// the agent ran a turn; `Err(message)` is the projected failure.
        result: Result<AgentRun, String>,
    },
    /// The runner is about to tear down the workspace.
    WorkspaceTeardownStarting {
        /// Signal currently being handled.
        signal: SharedSignal,
        /// Workspace path that will be torn down.
        path: PathBuf,
    },
    /// The workspace teardown finished.
    ///
    /// Carries the `path` of the (now torn-down) workspace because this is
    /// the canonical place for commit-on-teardown shell handlers to run
    /// `git` commands — and they need a cwd, not just a signal id.
    WorkspaceTeardownFinished {
        /// Signal currently being handled.
        signal: SharedSignal,
        /// Filesystem path of the workspace that was torn down.
        path: PathBuf,
    },
    /// A dequeue operation failed.
    DequeueFailed {
        /// Stringified error message.
        error: String,
    },
    /// Prompt rendering failed for a signal.
    RenderPromptFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Workspace setup failed for a signal.
    WorkspaceSetupFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Agent run failed for a signal.
    AgentRunFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// Workspace teardown failed for a signal.
    WorkspaceTeardownFailed {
        /// Signal being handled when the error occurred.
        signal_id: SignalId,
        /// Stringified error message.
        error: String,
    },
    /// The runner has finished its per-signal loop and is about to return
    /// from [`Runner::run`](crate::runner::Runner::run). Fired exactly once
    /// regardless of termination reason — including `RunnerExitError` exit
    /// paths.
    RunnerFinished {
        /// Why the runner loop terminated.
        reason: RunnerTerminationReason,
        /// Number of signals processed (whether successfully or not).
        iteration_count: u32,
    },
}

/// Routing key for event dispatch.
///
/// Each variant names a logical event the runner emits. The emitter
/// uses this to invoke only the handlers registered for a given name
/// rather than broadcasting to all handlers.
///
/// The mapping from [`Event`] to `EventName` is defined by
/// [`Event::name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventName {
    /// `runner_starting`
    RunnerStarting,
    /// `signal_received`
    SignalReceived,
    /// `workspace_setup_starting`
    WorkspaceSetupStarting,
    /// `workspace_setup_finished`
    WorkspaceSetupFinished,
    /// `agent_starting`
    AgentStarting,
    /// `agent_finished`
    AgentFinished,
    /// `workspace_teardown_starting`
    WorkspaceTeardownStarting,
    /// `workspace_teardown_finished`
    WorkspaceTeardownFinished,
    /// `runner_error` — covers all error variants.
    RunnerError,
    /// `runner_finished`
    RunnerFinished,
}

impl EventName {
    /// All event name variants.
    pub const ALL: &'static [EventName] = &[
        EventName::RunnerStarting,
        EventName::SignalReceived,
        EventName::WorkspaceSetupStarting,
        EventName::WorkspaceSetupFinished,
        EventName::AgentStarting,
        EventName::AgentFinished,
        EventName::WorkspaceTeardownStarting,
        EventName::WorkspaceTeardownFinished,
        EventName::RunnerError,
        EventName::RunnerFinished,
    ];
}

impl Event {
    /// The routing key for this event.
    ///
    /// All error variants (`DequeueFailed`, `RenderPromptFailed`,
    /// `WorkspaceSetupFailed`, `AgentRunFailed`,
    /// `WorkspaceTeardownFailed`) map to [`EventName::RunnerError`].
    #[must_use]
    pub fn name(&self) -> EventName {
        match self {
            Self::RunnerStarting {} => EventName::RunnerStarting,
            Self::SignalReceived { .. } => EventName::SignalReceived,
            Self::WorkspaceSetupStarting { .. } => EventName::WorkspaceSetupStarting,
            Self::WorkspaceSetupFinished { .. } => EventName::WorkspaceSetupFinished,
            Self::AgentStarting { .. } => EventName::AgentStarting,
            Self::AgentFinished { .. } => EventName::AgentFinished,
            Self::WorkspaceTeardownStarting { .. } => EventName::WorkspaceTeardownStarting,
            Self::WorkspaceTeardownFinished { .. } => EventName::WorkspaceTeardownFinished,
            Self::DequeueFailed { .. }
            | Self::RenderPromptFailed { .. }
            | Self::WorkspaceSetupFailed { .. }
            | Self::AgentRunFailed { .. }
            | Self::WorkspaceTeardownFailed { .. } => EventName::RunnerError,
            Self::RunnerFinished { .. } => EventName::RunnerFinished,
        }
    }
}

impl Event {
    /// Return the signal id associated with this event, if any.
    #[must_use]
    pub fn signal_id(&self) -> Option<SignalId> {
        match self {
            Self::SignalReceived { signal }
            | Self::WorkspaceSetupStarting { signal }
            | Self::WorkspaceSetupFinished { signal, .. }
            | Self::AgentStarting { signal, .. }
            | Self::AgentFinished { signal, .. }
            | Self::WorkspaceTeardownStarting { signal, .. }
            | Self::WorkspaceTeardownFinished { signal, .. } => Some(signal.id()),
            Self::RenderPromptFailed { signal_id, .. }
            | Self::WorkspaceSetupFailed { signal_id, .. }
            | Self::AgentRunFailed { signal_id, .. }
            | Self::WorkspaceTeardownFailed { signal_id, .. } => Some(*signal_id),
            Self::DequeueFailed { .. } | Self::RunnerStarting {} | Self::RunnerFinished { .. } => {
                None
            }
        }
    }

    /// Borrow the signal this event carries, if any.
    ///
    /// Returns `None` for events with no signal context
    /// (`RunnerStarting`, `RunnerFinished`, `DequeueFailed`) and for the
    /// error variants, which retain only a `signal_id` — the full signal is
    /// not kept past the failure path. The returned `&Signal` is borrowed out
    /// of the shared [`SharedSignal`] handle, so consumers inspect signal
    /// context without depending on the storage representation.
    ///
    /// Use [`Event::signal_id`] when only the id is needed: it covers the
    /// error variants too, whereas this accessor does not.
    #[must_use]
    pub fn signal(&self) -> Option<&Signal> {
        match self {
            Self::SignalReceived { signal }
            | Self::WorkspaceSetupStarting { signal }
            | Self::WorkspaceSetupFinished { signal, .. }
            | Self::AgentStarting { signal, .. }
            | Self::AgentFinished { signal, .. }
            | Self::WorkspaceTeardownStarting { signal, .. }
            | Self::WorkspaceTeardownFinished { signal, .. } => Some(signal.as_signal()),
            Self::RenderPromptFailed { .. }
            | Self::WorkspaceSetupFailed { .. }
            | Self::AgentRunFailed { .. }
            | Self::WorkspaceTeardownFailed { .. }
            | Self::DequeueFailed { .. }
            | Self::RunnerStarting {}
            | Self::RunnerFinished { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_events_share_one_signal_allocation() {
        use std::path::PathBuf;

        use crate::signal::{Metadata, MetadataKey, MetadataValue, Signal};

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("user").expect("key"),
            MetadataValue::String("alice".into()),
        );
        let shared = SharedSignal::new(Signal::new(metadata));
        let id = shared.id();

        // Two events for one bracket, each handed a cheap clone of the shared
        // handle — the pattern `RunnerEvents` follows per emission.
        let setup = Event::WorkspaceSetupFinished {
            signal: shared.clone(),
            path: PathBuf::from("/tmp/ws"),
        };
        let teardown = Event::WorkspaceTeardownStarting {
            signal: shared.clone(),
            path: PathBuf::from("/tmp/ws"),
        };

        // Borrowed accessors expose the same id and metadata...
        assert_eq!(setup.signal_id(), Some(id));
        assert_eq!(teardown.signal_id(), Some(id));
        assert_eq!(
            setup.signal().expect("signal").metadata(),
            teardown.signal().expect("signal").metadata(),
        );

        // ...because both events point at one allocation — no per-event deep
        // clone of the metadata map. Compared through the public `as_signal`
        // accessor (two `&Signal` from the same `Arc` are pointer-equal),
        // so the test never reaches into the private storage.
        match (&setup, &teardown) {
            (
                Event::WorkspaceSetupFinished { signal: a, .. },
                Event::WorkspaceTeardownStarting { signal: b, .. },
            ) => assert!(
                std::ptr::eq(a.as_signal(), b.as_signal()),
                "events for one bracket must share a single signal allocation",
            ),
            _ => unreachable!(),
        }
    }

    #[test]
    fn signal_accessors_are_none_for_signalless_variants() {
        use crate::signal::Signal;

        // Error variants keep only a `signal_id`; control events carry no
        // signal at all. `signal()` is `None` for both, while `signal_id()`
        // still recovers the id from the error variants.
        let id = Signal::synthesized().id();
        let failed = Event::RenderPromptFailed {
            signal_id: id,
            error: "boom".into(),
        };
        assert!(failed.signal().is_none());
        assert_eq!(failed.signal_id(), Some(id));

        let starting = Event::RunnerStarting {};
        assert!(starting.signal().is_none());
        assert!(starting.signal_id().is_none());
    }

    #[test]
    fn event_name_all_covers_every_variant() {
        let all_set: std::collections::HashSet<EventName> =
            EventName::ALL.iter().copied().collect();
        // Exhaustive match — adding a variant without listing it here
        // causes a compile error.
        for &name in EventName::ALL {
            match name {
                EventName::RunnerStarting
                | EventName::SignalReceived
                | EventName::WorkspaceSetupStarting
                | EventName::WorkspaceSetupFinished
                | EventName::AgentStarting
                | EventName::AgentFinished
                | EventName::WorkspaceTeardownStarting
                | EventName::WorkspaceTeardownFinished
                | EventName::RunnerError
                | EventName::RunnerFinished => {}
            }
        }
        assert_eq!(
            all_set.len(),
            EventName::ALL.len(),
            "ALL contains duplicates",
        );
        assert_eq!(
            EventName::ALL.len(),
            10,
            "EventName variant count changed — update ALL",
        );
    }
}
