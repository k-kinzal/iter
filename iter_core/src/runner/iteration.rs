//! [`IterationContext`] / [`IterationState`] — the runner-side surface for
//! the `iteration.*` placeholder root.
//!
//! [`IterationState`] is a mutable accumulator the [`Runner`](super::Runner)
//! holds across the per-signal loop: it remembers when the runner started,
//! the result of the previous iteration, and the current win/lose streak.
//! [`IterationContext`] is the immutable view rendered against
//! [`Template`](crate::template::Template) — it's what the user actually
//! sees as `{{iteration.count}}`, `{{iteration.previous_result}}`, and so
//! on.
//!
//! The `iteration.*` root deliberately exposes fixed fields only — there is
//! no user-defined state in v1, no persistence, and no commands to mutate it
//! from a hook. Anything you can see here, the runner already knew; we are
//! just naming it.
//!
//! `count` is **1-indexed at render time**. The first iteration sees
//! `iteration.count == 1`, so `iteration.count % 10 == 0` fires on
//! iterations 10, 20, 30… as a human would expect.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::signal::SignalId;

/// Result category attached to [`IterationState::record_success`] /
/// [`IterationState::record_failure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviousResult {
    /// No previous iteration has been recorded yet (first iteration).
    None,
    /// Previous iteration completed without error or non-zero agent exit
    /// (per the runner's success path).
    Success,
    /// Previous iteration recorded a processing error or a non-zero /
    /// signal-terminated agent exit.
    Errored,
}

impl PreviousResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Success => "success",
            Self::Errored => "errored",
        }
    }
}

/// Mutable accumulator the runner threads through its per-signal loop.
///
/// On every iteration the runner snapshots the state into an
/// [`IterationContext`] before rendering the prompt and dispatching
/// events; after the agent finishes (or an operation fails) it calls
/// [`Self::record_success`] / [`Self::record_failure`] before incrementing
/// `iteration_count`.
#[derive(Debug, Clone)]
pub struct IterationState {
    runner_started_at: DateTime<Utc>,
    current_iteration_started_at: DateTime<Utc>,
    previous_result: PreviousResult,
    previous_exit_code: Option<i32>,
    previous_signal_id: Option<SignalId>,
    previous_finished_at: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    consecutive_successes: u32,
}

impl IterationState {
    /// Build a fresh state at runner start. `runner_started_at` is the
    /// moment the runner entered its loop — it is preserved verbatim
    /// across every iteration as `iteration.runner_started_at`.
    #[must_use]
    pub fn new(runner_started_at: DateTime<Utc>) -> Self {
        Self {
            runner_started_at,
            current_iteration_started_at: runner_started_at,
            previous_result: PreviousResult::None,
            previous_exit_code: None,
            previous_signal_id: None,
            previous_finished_at: None,
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }

    /// Mark the start of a new iteration. The runner calls this just
    /// after dequeuing or synthesising the signal, before rendering the
    /// prompt — so `iteration.started_at` reflects when *this* iteration
    /// began, not when the prior one finished.
    pub fn begin_iteration(&mut self, started_at: DateTime<Utc>) {
        self.current_iteration_started_at = started_at;
    }

    /// Record a successful iteration. Bumps the success streak and
    /// clears the failure streak.
    pub fn record_success(
        &mut self,
        signal_id: SignalId,
        exit_code: Option<i32>,
        finished_at: DateTime<Utc>,
    ) {
        self.previous_result = PreviousResult::Success;
        self.previous_exit_code = exit_code;
        self.previous_signal_id = Some(signal_id);
        self.previous_finished_at = Some(finished_at);
        self.consecutive_successes = self.consecutive_successes.saturating_add(1);
        self.consecutive_failures = 0;
    }

    /// Record an errored iteration. Bumps the failure streak and clears
    /// the success streak. `exit_code` is `None` for errors that did not
    /// surface a process exit code.
    pub fn record_failure(
        &mut self,
        signal_id: SignalId,
        exit_code: Option<i32>,
        finished_at: DateTime<Utc>,
    ) {
        self.previous_result = PreviousResult::Errored;
        self.previous_exit_code = exit_code;
        self.previous_signal_id = Some(signal_id);
        self.previous_finished_at = Some(finished_at);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.consecutive_successes = 0;
    }

    /// Build the rendering view for the iteration whose count is
    /// `count`. `count` is the 1-indexed number of the iteration about
    /// to run (i.e. `iteration_count + 1` from the runner's local
    /// counter).
    #[must_use]
    pub fn snapshot(&self, count: u32) -> IterationContext {
        IterationContext {
            count,
            started_at: self.current_iteration_started_at,
            runner_started_at: self.runner_started_at,
            previous_result: self.previous_result,
            previous_exit_code: self.previous_exit_code,
            previous_signal_id: self.previous_signal_id,
            consecutive_failures: self.consecutive_failures,
            consecutive_successes: self.consecutive_successes,
        }
    }

    /// Borrow the latest recorded result category.
    #[must_use]
    pub fn previous_result(&self) -> PreviousResult {
        self.previous_result
    }

    /// Borrow the latest recorded process exit code, when any.
    #[must_use]
    pub fn previous_exit_code(&self) -> Option<i32> {
        self.previous_exit_code
    }

    /// Borrow the running failure-streak counter.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Borrow the running success-streak counter.
    #[must_use]
    pub fn consecutive_successes(&self) -> u32 {
        self.consecutive_successes
    }
}

/// Serializable view of [`IterationState`] for [`Template`](crate::template::Template) rendering.
///
/// Field-by-field shape (every field is reachable from a template as
/// `{{iteration.<name>}}`):
///
/// * `count` — 1-indexed iteration number.
/// * `started_at` / `runner_started_at` — RFC 3339 timestamps.
/// * `previous_result` — `"none" | "success" | "errored"`.
/// * `previous_exit_code` — process exit code or `null`. Templates that
///   reference it without a previous iteration surface a strict-mode error.
/// * `previous_signal_id` — UUID v7 of the previous signal or `null`.
/// * `consecutive_failures` / `consecutive_successes` — streak counters.
///
/// The view is constructed by [`IterationState::snapshot`] and used as a
/// child of [`IterationRenderContext`](crate::template::context::IterationRenderContext).
#[derive(Debug, Clone, Serialize)]
pub struct IterationContext {
    /// 1-indexed iteration number for the iteration currently rendering.
    pub count: u32,
    /// Wall-clock instant the current iteration began.
    #[serde(serialize_with = "serialize_rfc3339")]
    pub started_at: DateTime<Utc>,
    /// Wall-clock instant the runner entered its loop.
    #[serde(serialize_with = "serialize_rfc3339")]
    pub runner_started_at: DateTime<Utc>,
    /// Result category of the previous iteration; `None` on the first.
    /// Rendered in templates/guards as `iteration.previous_result`.
    pub previous_result: PreviousResult,
    /// Process exit code recorded for the previous iteration. Under the
    /// `Ok = ran` model this is the normalized success value `Some(0)`
    /// after a clean run; on a failed run it carries the code the agent
    /// `Err` reported (`AgentError::Failed { code }`), or `None` for failures
    /// without a process exit code (signal termination, launch failure,
    /// cancellation, timeout, token limit).
    pub previous_exit_code: Option<i32>,
    /// Signal identifier of the previous iteration, when one was
    /// available.
    pub previous_signal_id: Option<SignalId>,
    /// Number of consecutive failed iterations leading up to this one.
    pub consecutive_failures: u32,
    /// Number of consecutive successful iterations leading up to this
    /// one.
    pub consecutive_successes: u32,
}

impl IterationContext {
    /// Convenience: produce a deterministic, plausible context for tests
    /// that exercise prompt/guard rendering. Counts as iteration 1 with
    /// no previous result. Crate-public rather than `#[cfg(test)]`
    /// because downstream crates' integration tests construct contexts
    /// through this surface as well.
    #[must_use]
    pub fn for_test() -> Self {
        Self::for_count(1)
    }

    /// Build a deterministic context with an explicit `count`. Mirrors
    /// `for_test` but lets tests target a specific iteration number,
    /// which matters for guard tests around `iteration.count % N`.
    ///
    /// The returned context is built from a fresh
    /// [`IterationState`] — there is no simulated prior result
    /// (`previous_result == "none"`, both streak counters `0`,
    /// `previous_exit_code == None`, `previous_signal_id == None`).
    /// Tests that need to exercise `previous_result == "errored"` /
    /// `"success"` or specific streak values must build their own
    /// `IterationState`, drive `record_success` / `record_failure`,
    /// and call `snapshot(count)` directly.
    #[must_use]
    pub fn for_count(count: u32) -> Self {
        let now = Utc::now();
        IterationState::new(now).snapshot(count)
    }

    /// Project the result string a template renders for
    /// `{{iteration.previous_result}}`.
    #[must_use]
    pub fn previous_result_str(&self) -> &'static str {
        self.previous_result.as_str()
    }
}

fn serialize_rfc3339<S: serde::Serializer>(
    ts: &DateTime<Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&ts.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::Signal;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn fresh_state_has_no_previous_result() {
        let state = IterationState::new(now());
        let snap = state.snapshot(1);
        assert_eq!(snap.count, 1);
        assert_eq!(snap.previous_result, PreviousResult::None);
        assert_eq!(snap.previous_exit_code, None);
        assert_eq!(snap.previous_signal_id, None);
        assert_eq!(snap.consecutive_failures, 0);
        assert_eq!(snap.consecutive_successes, 0);
    }

    #[test]
    fn record_success_bumps_success_streak_and_resets_failures() {
        let mut state = IterationState::new(now());
        let signal = Signal::synthesized();
        state.record_failure(signal.id(), Some(2), now());
        state.record_failure(signal.id(), Some(3), now());
        assert_eq!(state.consecutive_failures(), 2);

        state.record_success(signal.id(), Some(0), now());
        assert_eq!(state.consecutive_failures(), 0);
        assert_eq!(state.consecutive_successes(), 1);
        assert_eq!(state.previous_result(), PreviousResult::Success);
    }

    #[test]
    fn record_failure_bumps_failure_streak_and_resets_successes() {
        let mut state = IterationState::new(now());
        let signal = Signal::synthesized();
        state.record_success(signal.id(), Some(0), now());
        state.record_success(signal.id(), Some(0), now());
        assert_eq!(state.consecutive_successes(), 2);

        state.record_failure(signal.id(), Some(1), now());
        assert_eq!(state.consecutive_failures(), 1);
        assert_eq!(state.consecutive_successes(), 0);
        assert_eq!(state.previous_result(), PreviousResult::Errored);
    }

    #[test]
    fn snapshot_count_is_one_indexed_for_first_iteration() {
        let state = IterationState::new(now());
        let snap = state.snapshot(1);
        assert_eq!(snap.count, 1);
    }

    #[test]
    fn snapshot_carries_previous_signal_id_after_record() {
        let mut state = IterationState::new(now());
        let signal = Signal::synthesized();
        state.record_success(signal.id(), Some(0), now());
        let snap = state.snapshot(2);
        assert_eq!(snap.previous_signal_id, Some(signal.id()));
        assert_eq!(snap.previous_result, PreviousResult::Success);
        assert_eq!(snap.previous_exit_code, Some(0));
    }

    #[test]
    fn begin_iteration_updates_started_at() {
        let runner_start = now();
        let mut state = IterationState::new(runner_start);
        let later = runner_start + chrono::Duration::seconds(7);
        state.begin_iteration(later);
        let snap = state.snapshot(1);
        assert_eq!(snap.started_at, later);
        assert_eq!(snap.runner_started_at, runner_start);
    }

    #[test]
    fn previous_result_serializes_as_snake_case() {
        let snap = IterationContext::for_count(1);
        let json = serde_json::to_value(&snap).expect("serialize");
        assert_eq!(json["previous_result"], "none");
    }

    #[test]
    fn previous_signal_id_serializes_as_uuid_string() {
        let mut state = IterationState::new(now());
        let signal = Signal::synthesized();
        state.record_success(signal.id(), Some(0), now());
        let snap = state.snapshot(2);
        let json = serde_json::to_value(&snap).expect("serialize");
        assert_eq!(json["previous_signal_id"], signal.id().to_string());
    }

    #[test]
    fn streak_after_alternating_results_resets_each_time() {
        let mut state = IterationState::new(now());
        let signal = Signal::synthesized();
        state.record_success(signal.id(), Some(0), now());
        state.record_failure(signal.id(), Some(1), now());
        state.record_success(signal.id(), Some(0), now());
        assert_eq!(state.consecutive_successes(), 1);
        assert_eq!(state.consecutive_failures(), 0);
    }
}
