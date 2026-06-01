//! Builder for [`Runner`](super::Runner).
//!
//! `Runner` has many fields and several of them are required, so we use a
//! builder type to make construction explicit and to surface meaningful
//! errors when something is missing.

use std::sync::Arc;

use crate::agent::Agent;
use crate::prompt::{PromptSelector, PromptTemplate};
use crate::queue::Queue;
use crate::runner::event::EventName;
use crate::runner::observer::{DynRunnerObserver, RunnerObserver};
use crate::runner::{EventEmitter, EventHandler, Runner, RunnerBehavior, RunnerConfig};
use crate::workspace::Workspace;

/// Errors emitted by [`RunnerBuilder::build`].
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// A required field was not supplied to the builder.
    #[error("missing required field: {0}")]
    MissingField(&'static str),

    /// The supplied configuration is internally inconsistent.
    ///
    /// The canonical case is `(queue=None, behavior=Wait)`: there is
    /// nothing to wait on, so the runner cannot make progress. Switch to
    /// `behavior = loop` (which synthesises signals) or supply a queue.
    #[error("invalid configuration: {0}")]
    InvalidConfig(&'static str),
}

/// Fluent builder for [`Runner`].
#[must_use = "call `build()` to produce a Runner"]
pub struct RunnerBuilder<Q: Queue, W: Workspace, A: Agent> {
    queue: Option<Arc<Q>>,
    workspaces: Option<Arc<dyn Fn() -> W + Send + Sync>>,
    agent: Option<A>,
    prompt_selector: Option<PromptSelector>,
    events: EventEmitter,
    observers: Vec<Arc<dyn DynRunnerObserver>>,
    config: RunnerConfig,
    stdio_sink: Option<Arc<dyn crate::log::OutputSink>>,
}

impl<Q: Queue, W: Workspace, A: Agent> Default for RunnerBuilder<Q, W, A> {
    fn default() -> Self {
        Self {
            queue: None,
            workspaces: None,
            agent: None,
            prompt_selector: None,
            events: EventEmitter::new(),
            observers: Vec::new(),
            config: RunnerConfig::default(),
            stdio_sink: None,
        }
    }
}

impl<Q: Queue, W: Workspace, A: Agent> RunnerBuilder<Q, W, A> {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Supply the [`Queue`] the runner should pull signals from.
    ///
    /// Optional: a runner may operate without a queue when configured
    /// with `behavior = loop` (the runner synthesises signals on its own
    /// instead of pulling them from upstream). Combining
    /// `behavior = wait` with no queue is rejected at [`Self::build`]
    /// time because there is nothing to park on.
    pub fn queue(mut self, queue: Arc<Q>) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Supply the workspace factory used to mint fresh workspaces.
    pub fn workspaces<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> W + Send + Sync + 'static,
    {
        self.workspaces = Some(Arc::new(factory));
        self
    }

    /// Supply the [`Agent`] used for every iteration.
    pub fn agent(mut self, agent: A) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Supply the [`PromptSelector`] used to render prompts.
    ///
    /// Prefer this method when an Iterfile has declared guarded prompts;
    /// use [`RunnerBuilder::prompt_template`] as a shortcut when the
    /// caller only has a single unguarded template.
    pub fn prompt_selector(mut self, selector: PromptSelector) -> Self {
        self.prompt_selector = Some(selector);
        self
    }

    /// Convenience wrapper around [`RunnerBuilder::prompt_selector`] for
    /// the common case of a single, unguarded template. The template is
    /// stored as the selector's default branch.
    pub fn prompt_template(self, template: PromptTemplate) -> Self {
        self.prompt_selector(PromptSelector::single(template))
    }

    /// Replace the runner's [`RunnerConfig`].
    pub fn config(mut self, config: RunnerConfig) -> Self {
        self.config = config;
        self
    }

    /// Register an [`EventHandler`] for a specific [`EventName`].
    ///
    /// The handler is only invoked when the emitter dispatches an event
    /// whose [`Event::name`](crate::runner::Event::name) matches.
    pub fn on<H>(mut self, name: EventName, handler: H) -> Self
    where
        H: EventHandler + 'static,
    {
        self.events.on(name, handler);
        self
    }

    /// Register an [`EventHandler`] for every [`EventName`].
    ///
    /// The handler must be [`Clone`] because it is registered once per
    /// event name. Useful for test capture handlers and cross-cutting
    /// concerns like logging.
    #[allow(clippy::needless_pass_by_value)]
    pub fn on_all<H>(self, handler: H) -> Self
    where
        H: EventHandler + Clone + 'static,
    {
        let mut this = self;
        for &name in EventName::ALL {
            this.events.on(name, handler.clone());
        }
        this
    }

    /// Replace the [`EventEmitter`] wholesale.
    pub fn events(mut self, events: EventEmitter) -> Self {
        self.events = events;
        self
    }

    /// Register a [`RunnerObserver`] for the system observer stream.
    ///
    /// Observers run **before** the user-defined [`EventEmitter`] handlers
    /// at every runner step (rev17 §F3) so a user-installed
    /// `on workspace_teardown_finished { shell "..." }` cannot mask the system
    /// contract that backs `~/.iter/proc/<id>/log.ndjson`. Failures are
    /// best-effort — they are tallied into
    /// [`RunnerSummary::observer_error_count`](crate::RunnerSummary::observer_error_count)
    /// and logged via `tracing` at `warn` level, but do not halt the loop.
    pub fn observer<O>(mut self, observer: O) -> Self
    where
        O: RunnerObserver + 'static,
    {
        let erased: Arc<dyn DynRunnerObserver> = Arc::new(observer);
        self.observers.push(erased);
        self
    }

    /// Install the [`OutputSink`](crate::log::OutputSink) every
    /// agent invocation should tee its child stdout/stderr through. The
    /// `iter_compose` entry point wires this from
    /// `ProcessRuntime::sink()` so agent output reaches the per-process
    /// `log.ndjson`. Standalone runners may leave it unset — the runner
    /// falls back to a [`NoopSink`](crate::log::NoopSink) in that case.
    pub fn stdio_sink(mut self, sink: Arc<dyn crate::log::OutputSink>) -> Self {
        self.stdio_sink = Some(sink);
        self
    }

    /// Returns `true` when an [`OutputSink`](crate::log::OutputSink)
    /// has been installed via [`Self::stdio_sink`].
    #[must_use]
    pub fn has_stdio_sink(&self) -> bool {
        self.stdio_sink.is_some()
    }

    /// Returns `true` when at least one
    /// [`RunnerObserver`](crate::runner::observer::RunnerObserver) has
    /// been installed via [`Self::observer`].
    #[must_use]
    pub fn has_observer(&self) -> bool {
        !self.observers.is_empty()
    }

    /// Finish building, returning the [`Runner`] or a [`BuilderError`].
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// Every driver now uses [`AgentError`](crate::agent::AgentError)
    /// directly — the `Agent` trait has no associated error type — so this
    /// builder needs no extra bound beyond the struct's `A: Agent`.
    pub fn build(self) -> Result<Runner<Q, W, A>, BuilderError> {
        let workspaces = self
            .workspaces
            .ok_or(BuilderError::MissingField("workspaces"))?;
        let agent = self.agent.ok_or(BuilderError::MissingField("agent"))?;
        let prompt_selector = self
            .prompt_selector
            .ok_or(BuilderError::MissingField("prompt_selector"))?;

        if self.queue.is_none() && matches!(self.config.behavior, RunnerBehavior::Wait) {
            return Err(BuilderError::InvalidConfig(
                "behavior = wait requires a queue declaration",
            ));
        }

        Ok(Runner {
            queue: self.queue,
            workspaces,
            agent,
            prompt_selector,
            events: self.events,
            observers: self.observers,
            config: self.config,
            stdio_sink: self
                .stdio_sink
                .unwrap_or_else(|| Arc::new(crate::log::NoopSink)),
        })
    }
}
