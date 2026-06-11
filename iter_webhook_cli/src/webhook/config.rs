//! Configuration and error types for [`WebhookTrigger`](super::WebhookTrigger).

use std::net::SocketAddr;

use iter_core::template::{Template, TemplateError};
use iter_core::{MetadataError, MetadataKey, Priority};
use thiserror::Error;

/// Top-level [`WebhookTrigger`](super::WebhookTrigger) configuration.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Address the HTTP server should bind to.
    pub bind: SocketAddr,
    /// Path the server should accept POSTs on, e.g. `"/github"`.
    pub path: String,
    /// Optional HMAC secret. When set, requests must include a valid
    /// `X-Hub-Signature-256` header computed over the raw body.
    pub secret: Option<String>,
    /// Routes evaluated against each incoming event.
    pub routes: Vec<Subscription>,
}

/// One routing rule attached to a [`WebhookConfig`].
///
/// The `metadata` entries are raw template source strings as they appeared
/// in the config. [`WebhookTrigger::new`](super::WebhookTrigger::new)
/// compiles them once into [`Template`] instances held in the internal
/// [`CompiledRoute`].
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Pattern matched against `<event>.<action>` (e.g. `"issues.opened"`).
    /// `*` is a wildcard for either side, e.g. `"issues.*"`.
    pub event_pattern: String,
    /// Optional guard expression. The v1 implementation supports the form
    /// `{{event.path.to.field}} == 'literal'` and treats anything else as
    /// "always true".
    pub when: Option<String>,
    /// Priority used when enqueuing the rendered signal.
    pub priority: Priority,
    /// Metadata template list. Each value may contain `{{event.x.y}}`
    /// placeholders rendered against the request body.
    pub metadata: Vec<(String, String)>,
}

/// Internal form of a [`Subscription`] with metadata templates compiled.
///
/// Built once by [`WebhookTrigger::new`](super::WebhookTrigger::new) and
/// shared read-only via the axum [`WebhookState`](super::router::WebhookState).
#[derive(Debug, Clone)]
pub(super) struct CompiledRoute {
    pub(super) event_pattern: String,
    pub(super) when: Option<String>,
    pub(super) priority: Priority,
    pub(super) metadata: Vec<(MetadataKey, Template)>,
}

impl CompiledRoute {
    pub(super) fn from_route(route: &Subscription) -> Result<Self, CompileRouteError> {
        let mut metadata = Vec::with_capacity(route.metadata.len());
        for (key, template_source) in &route.metadata {
            let key = MetadataKey::new(key.as_str())?;
            let template = Template::compile(template_source.clone())?;
            metadata.push((key, template));
        }
        Ok(Self {
            event_pattern: route.event_pattern.clone(),
            when: route.when.clone(),
            priority: route.priority,
            metadata,
        })
    }
}

/// Errors produced while compiling a [`Subscription`] into a
/// [`CompiledRoute`].
#[derive(Debug, Error)]
pub(super) enum CompileRouteError {
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error(transparent)]
    Template(#[from] TemplateError),
}

/// Errors produced by [`WebhookTrigger`](super::WebhookTrigger).
#[derive(Debug, Error)]
pub enum WebhookTriggerError<E: std::error::Error + Send + Sync + 'static> {
    /// Forwarded error from the queue backing the trigger.
    #[error("queue error: {0}")]
    #[allow(dead_code)]
    Queue(#[source] E),

    /// I/O error binding or accepting connections.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Construction of an internal metadata key failed.
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),

    /// A route metadata template failed to compile.
    #[error("template error: {0}")]
    Template(#[from] TemplateError),

    /// The internal axum server failed.
    #[error("server error: {0}")]
    Server(String),
}

impl<E: std::error::Error + Send + Sync + 'static> From<CompileRouteError>
    for WebhookTriggerError<E>
{
    fn from(err: CompileRouteError) -> Self {
        match err {
            CompileRouteError::Metadata(e) => Self::Metadata(e),
            CompileRouteError::Template(e) => Self::Template(e),
        }
    }
}
