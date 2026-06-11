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

use crate::runner::iteration::IterationContext;
use crate::signal::Signal;
use crate::signal::metadata::MetadataValue;

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
}

impl<'a> IterationRenderContext<'a> {
    /// Build a render context for `signal` and `iteration`.
    #[must_use]
    pub fn new(signal: &'a Signal, iteration: &'a IterationContext) -> Self {
        Self {
            signal: SignalContext::from_signal(signal),
            iteration,
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
}

impl<'a> RunnerRenderContext<'a> {
    /// Build a lifecycle render context anchored on `iteration`.
    #[must_use]
    pub fn new(iteration: &'a IterationContext) -> Self {
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        Self { today, iteration }
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
}
