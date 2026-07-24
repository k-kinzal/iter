//! Serializable context types used as input to [`Template::render`](super::Template::render).
//!
//! [`SignalContext`] is the shape both [`PromptTemplate`](crate::PromptTemplate)
//! and the shell action render against. It exposes
//! `{{today}}`, `{{signal.id}}`, `{{signal.created_at}}`, and every
//! `{{metadata.*}}` key attached to a [`Signal`].
//!
//! [`IterationRenderContext`] composes [`SignalContext`] with an
//! [`IterationContext`](crate::IterationContext) so prompts and shell
//! handlers attached to per-signal events can additionally render
//! `{{iteration.count}}`, `{{iteration.previous_result}}`, and so on.
//! [`RunnerRenderContext`] is the signal-less twin used for runner-level
//! events (`runner_starting`, `runner_finished`, `runner_error` without a
//! signal in flight) — `{{iteration.*}}` is reachable but `{{signal.*}}`
//! and `{{metadata.*}}` are not.

use std::collections::BTreeMap;

use chrono::{DateTime, Local, Utc};
use serde::Serialize;

use crate::AgentRun;
use crate::runner::completion::{CompletionConditionInfo, CompletionEvent};
use crate::runner::iteration::IterationContext;
use crate::signal::Signal;
use crate::signal::metadata::MetadataValue;
use crate::variable::VariableSnapshot;

/// Serializable view of a [`Signal`] for Handlebars rendering.
///
/// Fields:
/// * `today` — local date formatted as `YYYY-MM-DD`.
/// * `signal.id` — canonical UUID v7 string.
/// * `signal.created_at` — RFC 3339 timestamp.
/// * `metadata.<key>` — each present metadata key; missing keys surface
///   as Handlebars strict-mode "missing variable" errors.
#[derive(Debug, Serialize)]
pub struct SignalContext<'a> {
    today: String,
    signal: SignalView<'a>,
    metadata: BTreeMap<&'a str, String>,
}

#[derive(Debug, Serialize)]
struct SignalView<'a> {
    id: String,
    created_at: String,
    #[serde(skip)]
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> SignalContext<'a> {
    /// Build a render context from `signal`.
    #[must_use]
    pub fn from_signal(signal: &'a Signal) -> Self {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let created_at: DateTime<Utc> = signal.created_at();
        let signal_view = SignalView {
            id: signal.id().to_string(),
            created_at: created_at.to_rfc3339(),
            _phantom: std::marker::PhantomData,
        };
        let mut metadata: BTreeMap<&'a str, String> = BTreeMap::new();
        for (key, value) in signal.metadata() {
            metadata.insert(key.as_str(), metadata_value_to_string(value));
        }
        Self {
            today,
            signal: signal_view,
            metadata,
        }
    }
}

/// Combined view of a [`Signal`] and the runner's
/// [`IterationContext`](crate::IterationContext) used by per-signal
/// render paths (prompts, `on agent_starting`, `on workspace_setup_*`,
/// etc.). `signal`, `today`, and `metadata.*` come from the embedded
/// [`SignalContext`] via `#[serde(flatten)]` so existing templates stay
/// unchanged; `iteration.*` is added as a new top-level root.
#[derive(Debug, Serialize)]
pub struct IterationRenderContext<'a> {
    #[serde(flatten)]
    signal: SignalContext<'a>,
    iteration: &'a IterationContext,
    var: VariableSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<&'a AgentRun>,
}

impl<'a> IterationRenderContext<'a> {
    /// Build a render context for `signal` and `iteration`.
    #[must_use]
    pub fn new(signal: &'a Signal, iteration: &'a IterationContext) -> Self {
        Self {
            signal: SignalContext::from_signal(signal),
            iteration,
            var: VariableSnapshot::default(),
            agent: None,
        }
    }

    /// Build a render context with one point-in-time `var.*` snapshot.
    #[must_use]
    pub fn with_variables(
        signal: &'a Signal,
        iteration: &'a IterationContext,
        var: VariableSnapshot,
    ) -> Self {
        Self {
            signal: SignalContext::from_signal(signal),
            iteration,
            var,
            agent: None,
        }
    }

    /// Build the render view for `agent_finished`, including the successful
    /// [`AgentRun`] under `agent.*`.
    #[must_use]
    pub fn with_agent_and_variables(
        signal: &'a Signal,
        iteration: &'a IterationContext,
        var: VariableSnapshot,
        agent: &'a AgentRun,
    ) -> Self {
        Self {
            signal: SignalContext::from_signal(signal),
            iteration,
            var,
            agent: Some(agent),
        }
    }

    /// Borrow the embedded [`IterationContext`].
    #[must_use]
    pub fn iteration(&self) -> &IterationContext {
        self.iteration
    }
}

/// Render context for runner-level lifecycle events that have no signal
/// in flight (`runner_starting`, `runner_finished`, dequeue-level
/// `runner_error`). Templates here can reference `{{today}}` and the
/// full `{{iteration.*}}` root, but **not** `{{signal.*}}` or
/// `{{metadata.*}}` — strict mode surfaces a rendering error if they do.
#[derive(Debug, Serialize)]
pub struct RunnerRenderContext<'a> {
    today: String,
    iteration: &'a IterationContext,
    var: VariableSnapshot,
}

impl<'a> RunnerRenderContext<'a> {
    /// Build a lifecycle render context anchored on `iteration`.
    #[must_use]
    pub fn new(iteration: &'a IterationContext) -> Self {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        Self {
            today,
            iteration,
            var: VariableSnapshot::default(),
        }
    }

    /// Build a signal-less lifecycle context with a `var.*` snapshot.
    #[must_use]
    pub fn with_variables(iteration: &'a IterationContext, var: VariableSnapshot) -> Self {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        Self {
            today,
            iteration,
            var,
        }
    }
}

/// Render context for `runner_completing` and `runner_completed`.
///
/// It adds `completion.*` and `runner.*` roots without inventing a current
/// `signal.*` root for idle time-based completion.
#[derive(Debug, Serialize)]
pub struct CompletionRenderContext<'a> {
    today: String,
    iteration: &'a IterationContext,
    completion: CompletionView<'a>,
    runner: CompletionRunnerView,
    var: VariableSnapshot,
}

#[derive(Debug, Serialize)]
struct CompletionView<'a> {
    condition: &'a CompletionConditionInfo,
    requested_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompletionRunnerView {
    started_at: String,
    elapsed_seconds: u64,
    last_signal_id: Option<String>,
}

impl<'a> CompletionRenderContext<'a> {
    /// Build a completion lifecycle render context.
    #[must_use]
    pub fn new(event: &'a CompletionEvent, iteration: &'a IterationContext) -> Self {
        Self {
            today: Local::now().date_naive().format("%Y-%m-%d").to_string(),
            iteration,
            completion: CompletionView {
                condition: &event.request.condition,
                requested_at: event.request.requested_at.to_rfc3339(),
                completed_at: event.completed_at.map(|value| value.to_rfc3339()),
            },
            runner: CompletionRunnerView {
                started_at: event.request.runner_started_at.to_rfc3339(),
                elapsed_seconds: event.request.elapsed_seconds,
                last_signal_id: event.request.last_signal_id.map(|id| id.to_string()),
            },
            var: VariableSnapshot::default(),
        }
    }

    /// Build a completion context with a `var.*` snapshot.
    #[must_use]
    pub fn with_variables(
        event: &'a CompletionEvent,
        iteration: &'a IterationContext,
        var: VariableSnapshot,
    ) -> Self {
        let mut context = Self::new(event, iteration);
        context.var = var;
        context
    }
}

/// Convert a [`MetadataValue`] to the string it should render as inside a
/// template. Note: deliberately distinct from [`MetadataValue`]'s `Display`
/// impl — that one emits the literal `"null"` for [`MetadataValue::Null`],
/// but prompts have always rendered a null metadata value as the empty
/// string.
fn metadata_value_to_string(value: &MetadataValue) -> String {
    match value {
        MetadataValue::String(s) => s.clone(),
        MetadataValue::Integer(n) => n.to_string(),
        MetadataValue::Bool(b) => b.to_string(),
        MetadataValue::Null => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{
        CompletionConditionInfo, CompletionConditionKind, CompletionEvent, CompletionRequest,
    };
    use crate::signal::metadata::{Metadata, MetadataKey};

    fn signal_with(metadata: Metadata) -> Signal {
        Signal::new(metadata)
    }

    #[test]
    fn context_today_is_yyyy_mm_dd() {
        let signal = signal_with(Metadata::new());
        let ctx = SignalContext::from_signal(&signal);
        assert_eq!(ctx.today.len(), 10);
        assert_eq!(ctx.today.as_bytes()[4], b'-');
        assert_eq!(ctx.today.as_bytes()[7], b'-');
    }

    #[test]
    fn context_null_metadata_is_empty_string() {
        let mut meta = Metadata::new();
        meta.insert(MetadataKey::new("n").unwrap(), MetadataValue::Null);
        let signal = signal_with(meta);
        let ctx = SignalContext::from_signal(&signal);
        assert_eq!(ctx.metadata.get("n"), Some(&String::new()));
    }

    #[test]
    fn context_renders_integer_and_bool_metadata() {
        let mut meta = Metadata::new();
        meta.insert(MetadataKey::new("n").unwrap(), MetadataValue::Integer(7));
        meta.insert(MetadataKey::new("b").unwrap(), MetadataValue::Bool(true));
        let signal = signal_with(meta);
        let ctx = SignalContext::from_signal(&signal);
        assert_eq!(ctx.metadata.get("n"), Some(&"7".to_owned()));
        assert_eq!(ctx.metadata.get("b"), Some(&"true".to_owned()));
    }

    #[test]
    fn context_only_contains_present_keys() {
        let mut meta = Metadata::new();
        meta.insert(
            MetadataKey::new("present").unwrap(),
            MetadataValue::String("value".into()),
        );
        let signal = signal_with(meta);
        let ctx = SignalContext::from_signal(&signal);
        assert!(ctx.metadata.contains_key("present"));
        assert!(!ctx.metadata.contains_key("missing"));
    }

    #[test]
    fn render_context_flattens_signal_and_adds_iteration_root() {
        let signal = signal_with(Metadata::new());
        let iteration = IterationContext::for_count(3);
        let ctx = IterationRenderContext::new(&signal, &iteration);
        let json = serde_json::to_value(&ctx).expect("serialize");
        // Existing roots remain at top level via `#[serde(flatten)]`.
        assert!(json.get("today").is_some());
        assert!(json.get("signal").is_some());
        assert!(json.get("metadata").is_some());
        // New `iteration` root sits alongside.
        assert_eq!(json["iteration"]["count"], 3);
        assert_eq!(json["iteration"]["previous_result"], "none");
        assert_eq!(json["var"], serde_json::json!({}));
    }

    #[test]
    fn render_context_exposes_variable_snapshot() {
        let signal = signal_with(Metadata::new());
        let iteration = IterationContext::for_count(3);
        let store = crate::variable::VariableStore::new();
        store.set("context", serde_json::json!({"value": {"foo": 1}}));
        let ctx = IterationRenderContext::with_variables(&signal, &iteration, store.snapshot());
        let json = serde_json::to_value(&ctx).expect("serialize");
        assert_eq!(json["var"]["context"]["value"]["foo"], 1);
    }

    #[test]
    fn lifecycle_context_has_no_signal_or_metadata_root() {
        let iteration = IterationContext::for_count(1);
        let ctx = RunnerRenderContext::new(&iteration);
        let json = serde_json::to_value(&ctx).expect("serialize");
        assert!(json.get("today").is_some());
        assert!(json.get("iteration").is_some());
        assert!(json.get("signal").is_none());
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn completion_context_exposes_completion_and_runner_roots() {
        let now = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let event = CompletionEvent::completed(
            CompletionRequest {
                condition: CompletionConditionInfo {
                    name: "budget".into(),
                    kind: CompletionConditionKind::Iterations,
                    max: Some(7),
                    duration_seconds: None,
                    at: None,
                },
                iteration_count: 7,
                last_signal_id: None,
                requested_at: now,
                runner_started_at: now,
                elapsed_seconds: 12,
            },
            now,
        );
        let iteration = IterationContext::for_count(7);
        let json = serde_json::to_value(CompletionRenderContext::new(&event, &iteration))
            .expect("serialize");
        assert_eq!(json["completion"]["condition"]["name"], "budget");
        assert_eq!(json["completion"]["condition"]["kind"], "iterations");
        assert_eq!(json["completion"]["completed_at"], now.to_rfc3339());
        assert_eq!(json["runner"]["elapsed_seconds"], 12);
        assert!(json.get("signal").is_none());
    }
}
