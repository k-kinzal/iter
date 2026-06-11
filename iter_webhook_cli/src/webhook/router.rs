//! HTTP request handler for the webhook trigger.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use iter_core::{Queue, Signal};
use serde_json::Value;
use tracing::Instrument;

use crate::trigger_util::hmac::verify_github_signature;

use super::config::CompiledRoute;
use super::guard::{evaluate_guard, event_pattern_matches, render_metadata};

pub(super) struct WebhookState<Q: Queue + ?Sized> {
    pub(super) queue: Arc<Q>,
    pub(super) secret: Option<String>,
    pub(super) routes: Vec<CompiledRoute>,
    pub(super) trigger_name: Option<String>,
}

pub(super) async fn handle_webhook<Q: Queue + ?Sized + 'static>(
    State(state): State<Arc<WebhookState<Q>>>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let trigger_name = state.trigger_name.clone();
    async move {
        if let Some(secret) = state.secret.as_deref() {
            let header = headers
                .get("X-Hub-Signature-256")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !verify_github_signature(secret.as_bytes(), header, &body) {
                return (StatusCode::UNAUTHORIZED, "invalid signature").into_response();
            }
        }

        let value: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("invalid json body: {e}"))
                    .into_response();
            }
        };

        let event_kind = headers
            .get("X-GitHub-Event")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let action = value
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let combined = if action.is_empty() {
            event_kind.clone()
        } else {
            format!("{event_kind}.{action}")
        };

        let mut matches = 0u32;
        for route in &state.routes {
            if !event_pattern_matches(&route.event_pattern, &combined) {
                continue;
            }
            if let Some(guard) = route.when.as_deref()
                && !evaluate_guard(guard, &value)
            {
                continue;
            }
            let metadata = match render_metadata(route, &value) {
                Ok(m) => m,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("metadata render failed: {e}"),
                    )
                        .into_response();
                }
            };
            let signal = iter_core::telemetry::inject_current_context_into_signal(Signal::new(
                metadata,
            ));
            if let Err(e) = state.queue.enqueue(signal, route.priority).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("queue error: {e}"),
                )
                    .into_response();
            }
            matches += 1;
        }

        (StatusCode::OK, format!("matched {matches}")).into_response()
    }
    .instrument(tracing::info_span!(
        "iter.trigger.webhook.request",
        iter.trigger.kind = "webhook",
        iter.trigger.name = trigger_name.as_deref().unwrap_or(""),
    ))
    .await
}
