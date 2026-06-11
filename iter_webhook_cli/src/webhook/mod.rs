//! [`WebhookTrigger`] — receives HTTP webhooks (e.g., GitHub) and emits
//! signals matching configured routes.

mod config;
mod guard;
mod router;

pub use config::{Subscription, WebhookConfig, WebhookTriggerError};

use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use iter_core::Queue;
use tokio_util::sync::CancellationToken;

use config::CompiledRoute;
use router::{WebhookState, handle_webhook};

/// HTTP webhook trigger.
///
/// Spawns an axum server on `config.bind` that accepts `POST` requests on
/// `config.path`. For each accepted request the trigger:
///
/// 1. Verifies the `X-Hub-Signature-256` header against `config.secret` if
///    one is configured.
/// 2. Parses the body as JSON.
/// 3. Combines `X-GitHub-Event` and the body's `action` field into a
///    `<event>.<action>` key.
/// 4. For each matching route, evaluates the optional guard, renders the
///    metadata templates against the body, and enqueues a [`Signal`](iter_core::Signal).
///
/// The server runs in the calling task; cancellation triggers a graceful
/// shutdown via [`axum::serve::Serve::with_graceful_shutdown`].
pub struct WebhookTrigger<Q: Queue> {
    queue: Arc<Q>,
    bind: std::net::SocketAddr,
    path: String,
    secret: Option<String>,
    routes: Vec<CompiledRoute>,
    trigger_name: Option<String>,
}

impl<Q: Queue> std::fmt::Debug for WebhookTrigger<Q> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookTrigger")
            .field("bind", &self.bind)
            .field("path", &self.path)
            .field("routes", &self.routes.len())
            .field("trigger_name", &self.trigger_name)
            .finish_non_exhaustive()
    }
}

impl<Q: Queue + 'static> WebhookTrigger<Q> {
    /// Build a webhook trigger publishing to `queue`.
    ///
    /// All route metadata templates are compiled up-front. Template
    /// compilation errors surface here rather than on the first matching
    /// request.
    ///
    /// # Errors
    ///
    /// * [`WebhookTriggerError::Metadata`] — a route's metadata key is
    ///   not a valid [`MetadataKey`](iter_core::MetadataKey).
    /// * [`WebhookTriggerError::Template`] — a route's metadata value is
    ///   not a valid Handlebars template.
    pub fn new(queue: Arc<Q>, config: WebhookConfig) -> Result<Self, WebhookTriggerError<Q::Error>>
    where
        Q::Error: std::error::Error + Send + Sync + 'static,
    {
        let mut routes = Vec::with_capacity(config.routes.len());
        for route in &config.routes {
            routes.push(CompiledRoute::from_route(route)?);
        }
        Ok(Self {
            queue,
            bind: config.bind,
            path: config.path,
            secret: config.secret,
            routes,
            trigger_name: None,
        })
    }

    /// Attach the configured trigger name to emitted spans.
    #[must_use]
    pub fn with_trigger_name(mut self, name: impl Into<String>) -> Self {
        self.trigger_name = Some(name.into());
        self
    }

    /// Build the underlying axum router. Exposed for tests so they can call
    /// the handler without binding to a real socket.
    pub fn router(&self) -> Router {
        let state = Arc::new(WebhookState {
            queue: self.queue.clone(),
            secret: self.secret.clone(),
            routes: self.routes.clone(),
            trigger_name: self.trigger_name.clone(),
        });
        Router::new()
            .route(&self.path, post(handle_webhook::<Q>))
            .with_state(state)
    }

    /// Bind the HTTP listener and serve until the cancellation token fires.
    ///
    /// # Errors
    ///
    /// Returns `WebhookTriggerError` if binding or serving fails.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), WebhookTriggerError<Q::Error>> {
        let bind = self.bind;
        let router = self.router();

        let listener = tokio::net::TcpListener::bind(bind).await?;
        let server = axum::serve(listener, router)
            .with_graceful_shutdown(async move { cancel.cancelled().await });

        server
            .await
            .map_err(|e| WebhookTriggerError::Server(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use iter_core::Priority;
    use iter_core::queue::InMemoryQueue;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    fn test_config(routes: Vec<Subscription>) -> WebhookConfig {
        WebhookConfig {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            path: "/hook".into(),
            secret: None,
            routes,
        }
    }

    #[test]
    fn invalid_metadata_template_fails_at_new() {
        let queue = Arc::new(InMemoryQueue::new());
        let route = Subscription {
            event_pattern: "push".into(),
            when: None,
            priority: Priority::NORMAL,
            // Empty-expression `{{}}` fails to compile under handlebars.
            metadata: vec![("key".into(), "hello {{}}".into())],
        };
        let err = WebhookTrigger::new(queue, test_config(vec![route])).expect_err("must fail");
        assert!(
            matches!(err, WebhookTriggerError::Template(_)),
            "unexpected error variant: {err:?}"
        );
    }

    #[tokio::test]
    async fn missing_event_field_returns_500() {
        // Strict-mode handlebars rejects {{event.missing.field}} when the
        // payload does not carry it. The router should surface that as a
        // 500 response rather than silently rendering empty.
        let queue = Arc::new(InMemoryQueue::new());
        let route = Subscription {
            event_pattern: "push".into(),
            when: None,
            priority: Priority::NORMAL,
            metadata: vec![("repo".into(), "{{event.repository.full_name}}".into())],
        };
        let trigger = WebhookTrigger::new(queue, test_config(vec![route])).expect("build");
        let app = trigger.router();

        let request = Request::builder()
            .method("POST")
            .uri("/hook")
            .header("X-GitHub-Event", "push")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("build request");

        let response = app.oneshot(request).await.expect("router oneshot");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap_or_default();
        assert!(
            body.contains("metadata render failed"),
            "unexpected body: {body}"
        );
    }

    #[tokio::test]
    async fn matching_route_enqueues_signal() {
        let queue = Arc::new(InMemoryQueue::new());
        let route = Subscription {
            event_pattern: "push".into(),
            when: None,
            priority: Priority::NORMAL,
            metadata: vec![("repo".into(), "{{event.repository.full_name}}".into())],
        };
        let trigger = WebhookTrigger::new(queue.clone(), test_config(vec![route])).expect("build");
        let app = trigger.router();

        let request = Request::builder()
            .method("POST")
            .uri("/hook")
            .header("X-GitHub-Event", "push")
            .header("content-type", "application/json")
            .body(Body::from(
                "{\"repository\":{\"full_name\":\"octo/widget\"}}",
            ))
            .expect("build request");
        let response = app.oneshot(request).await.expect("router oneshot");
        assert_eq!(response.status(), StatusCode::OK);

        let cancel = CancellationToken::new();
        let signal = queue
            .dequeue(cancel.clone())
            .await
            .expect("dequeue ok")
            .expect("signal available");
        let value = signal.metadata().get_str("repo").expect("repo present");
        assert_eq!(value.to_string(), "octo/widget");
    }
}
