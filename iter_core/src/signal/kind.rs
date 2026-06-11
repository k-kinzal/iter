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
/// backend (in-memory, file, Redis, SQS, …) carries it
/// transparently.  Backends that pre-date this field will deserialize
/// existing messages as `Work` thanks to the `#[serde(default)]`
/// annotation on [`Signal::kind`](super::Signal).
///
/// # Forward compatibility
///
/// Signals are iter's long-term data plane for control messages between
/// triggers and runners, and `Terminate` is only the first such control
/// kind. Future runner/trigger semantics — synthetic, retry, pause, and the
/// like — are expected to arrive as *additional* typed variants here, never
/// as breaking enum expansions and never as out-of-band metadata
/// conventions bolted onto [`Signal`](super::Signal).
///
/// To make that contract explicit, `SignalKind` is `#[non_exhaustive]`.
/// Code inside `iter_core` may still match exhaustively (the `Display` impl
/// below does), so adding a variant remains a compiler-guided change in the
/// core. Consumers in other crates, however, must include a wildcard arm and
/// treat unrecognized kinds conservatively: a runner that does not understand
/// a control kind should leave the signal untouched rather than guess at its
/// meaning. Where a typed accessor exists — such as
/// [`Signal::is_terminate`](super::Signal::is_terminate) — prefer it over an
/// open-coded match.
///
/// Mind the scope of this guarantee: `#[non_exhaustive]` governs *source*
/// compatibility, letting new variants be added without breaking downstream
/// code that compiles against this crate. It does **not** by itself make a new
/// kind safe on the *wire*. The `#[serde(default)]` on
/// [`Signal::kind`](super::Signal) only rescues payloads that predate the
/// field — they decode as `Work`; a payload whose kind string an older binary
/// does not recognize still fails to deserialize. When a new variant is
/// introduced, decide its mixed-version wire behavior alongside it, together
/// with the runtime semantics that fix what handling it "conservatively" means
/// for that specific kind.
///
/// At the source level — the compatibility `#[non_exhaustive]` does provide —
/// downstream matching looks like this:
///
/// ```
/// use iter_core::SignalKind;
///
/// // Downstream code matches with a wildcard so a future kind cannot break
/// // the build:
/// fn disposition(kind: SignalKind) -> &'static str {
///     match kind {
///         SignalKind::Work => "run the agent pipeline",
///         SignalKind::Terminate => "stop the runner gracefully",
///         // A kind this build does not know about: do nothing surprising.
///         _ => "unknown control signal — ignore conservatively",
///     }
/// }
///
/// assert_eq!(disposition(SignalKind::Work), "run the agent pipeline");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
