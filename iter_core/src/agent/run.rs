//! [`AgentRun`] — iter's domain result for a single agent run.
//!
//! This is the **Agent level** of the three-layer agent stack (Command →
//! Driver/Adapter → Agent). It is intentionally minimal: it carries only
//! what iter itself consumes or what its exploration Factors read. The rich,
//! CLI-shaped result lives at the Command level (`drivers/<cli>/command.rs`)
//! and is projected down to this type by each driver acting as an Adapter.
//!
//! There is deliberately **no exit code** here. A successful [`AgentRun`]
//! means "the agent ran"; a non-zero / failed run is an
//! [`AgentError`](crate::agent::AgentError), not an `Ok` carrying a failure
//! field. iter assigns no task-meaning to an exit code, so the exit code
//! never crosses the Adapter boundary into this domain type.

use serde::{Deserialize, Serialize};

/// Result of one successful agent run, in iter's domain vocabulary.
///
/// Surfaced through
/// [`HookEvent::AgentFinished`](crate::runner::HookEvent::AgentFinished) so event
/// handlers and observers can correlate the run (e.g. against the session
/// it belongs to). The struct is `#[non_exhaustive]` so new Factor-relevant
/// fields can be added without breaking downstream construction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentRun {
    /// Session / conversation id reported by the underlying CLI, when it
    /// exposes one and the driver parsed it from the Command result. Feeds
    /// iter's session-log and continuous-context-persistence Factors, which
    /// key continuity off a stable session identity across runs.
    pub session_id: Option<String>,
}

impl AgentRun {
    /// A run that carries no correlation data. Used by drivers whose CLI has
    /// no machine-readable session identity (or whose mode does not surface
    /// one), and by the built-in agents.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A run carrying the session id the CLI reported.
    #[must_use]
    pub fn with_session_id(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
        }
    }
}
