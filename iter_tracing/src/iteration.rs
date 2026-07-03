//! Per-iteration correlation attributes carried on an implicit task-local
//! scope.
//!
//! OpenTelemetry's context model propagates correlation implicitly in-process;
//! this module extends that discipline to iter's iteration-scoped attributes
//! (`iter.signal.id` / `iter.signal.kind`) so they need not travel through
//! call signatures. The runner opens a scope around one iteration's future;
//! anything running inside it — prompt rendering, workspace setup, the agent
//! and its child-process environment injection — can read the attributes
//! without being handed them.
//!
//! The crate stays independent of every other iter crate, so the values are
//! carried in display form (`String`), not as `SignalId`/`SignalKind` types.
//!
//! # Scope semantics
//!
//! [`iteration_scope`] wraps exactly the future it is given; the attributes
//! are visible to everything polled *within* that future. They are **not
//! inherited by tasks detached with `tokio::spawn`** — a spawned task starts
//! a fresh task-local context. Callers that hand work to a spawned task and
//! still need the attributes must re-open the scope inside the task.
//! Concurrent scopes on separate tasks (for example compose services running
//! in one process) never observe each other's attributes.

/// Correlation attributes for one runner iteration, in display form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IterationAttrs {
    /// Display form of the signal id driving this iteration.
    pub signal_id: String,
    /// Display form of the signal kind driving this iteration.
    pub signal_kind: String,
}

impl IterationAttrs {
    /// Build attributes from anything displayable as the id and kind.
    #[must_use]
    pub fn new(signal_id: impl Into<String>, signal_kind: impl Into<String>) -> Self {
        Self {
            signal_id: signal_id.into(),
            signal_kind: signal_kind.into(),
        }
    }
}

tokio::task_local! {
    static CURRENT_ITERATION: IterationAttrs;
}

/// Run `fut` with `attrs` visible via [`current_iteration_attrs`] for its
/// whole extent.
///
/// See the [module docs](self) for the scope semantics — in particular, the
/// attributes do not follow work detached with `tokio::spawn`.
pub async fn iteration_scope<F: Future>(attrs: IterationAttrs, fut: F) -> F::Output {
    CURRENT_ITERATION.scope(attrs, fut).await
}

/// Read the current iteration's attributes, or `None` outside any
/// [`iteration_scope`].
#[must_use]
pub fn current_iteration_attrs() -> Option<IterationAttrs> {
    CURRENT_ITERATION.try_with(Clone::clone).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attrs_visible_inside_scope_and_absent_outside() {
        assert_eq!(current_iteration_attrs(), None);
        let observed = iteration_scope(IterationAttrs::new("sig-1", "work"), async {
            current_iteration_attrs()
        })
        .await;
        assert_eq!(observed, Some(IterationAttrs::new("sig-1", "work")));
        assert_eq!(current_iteration_attrs(), None);
    }

    #[tokio::test]
    async fn spawned_tasks_do_not_inherit_the_scope() {
        let observed = iteration_scope(IterationAttrs::new("sig-2", "work"), async {
            tokio::spawn(async { current_iteration_attrs() })
                .await
                .expect("spawned task")
        })
        .await;
        assert_eq!(observed, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_scopes_do_not_cross_contaminate() {
        let mut set = tokio::task::JoinSet::new();
        for i in 0..2 {
            set.spawn(async move {
                let id = format!("sig-{i}");
                iteration_scope(IterationAttrs::new(id.clone(), "work"), async move {
                    // Yield so the two scopes interleave on the runtime.
                    for _ in 0..16 {
                        tokio::task::yield_now().await;
                        let attrs = current_iteration_attrs().expect("inside scope");
                        assert_eq!(attrs.signal_id, id);
                    }
                })
                .await;
            });
        }
        while let Some(res) = set.join_next().await {
            res.expect("scoped task");
        }
    }
}
