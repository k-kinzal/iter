//! [`SignalKind`] — discriminates work signals from control signals.

use serde::{Deserialize, Serialize};

/// Distinguishes the purpose of a [`Signal`](super::Signal).
///
/// `Work` is the default and represents a normal signal that the runner
/// should process through its workspace/agent pipeline.  `Terminate`
/// tells the runner to exit gracefully without running the agent — it is
/// enqueued by a trigger (or any external producer) that wants the
/// runner to stop.
///
/// The kind travels inside the serialized signal payload, so every queue
/// backend (in-memory, file, Redis, SQS, Kafka, …) carries it
/// transparently.  Backends that pre-date this field will deserialize
/// existing messages as `Work` thanks to the `#[serde(default)]`
/// annotation on [`Signal::kind`](super::Signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// Normal work signal — the runner processes it through the full
    /// workspace → agent → teardown pipeline.
    #[default]
    Work,
    /// Termination signal — the runner exits gracefully without
    /// invoking the agent.
    Terminate,
}

impl std::fmt::Display for SignalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Work => "work",
            Self::Terminate => "terminate",
        };
        f.write_str(label)
    }
}
