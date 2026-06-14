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
use crate::runner::{EventAction, EventDispatcher, Runner, RunnerPolicy, SignalAcquisition};
use crate::time::{Clock, IdSource, SystemClock, SystemIdSource};
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
///
/// All three runtime axes are trait objects: the builder holds the queue as
/// `Arc<dyn Queue>`, the per-iteration workspace supply as
/// `Arc<dyn Fn() -> Box<dyn Workspace>>`, and the agent as `Box<dyn Agent>`
/// (R18 — a closed enum at the definition layer, a trait object at run time),
/// so `RunnerBuilder` carries no type parameters.
#[must_use = "call `build()` to produce a Runner"]
pub struct RunnerBuilder {
    queue: Option<Arc<dyn Queue>>,
    workspaces: Option<Arc<dyn Fn() -> Box<dyn Workspace> + Send + Sync>>,
    agent: Option<Box<dyn Agent>>,
    prompt_selector: Option<PromptSelector>,
    events: EventDispatcher,
    observers: Vec<Arc<dyn DynRunnerObserver>>,
    config: RunnerPolicy,
    stdio_sink: Option<Arc<dyn crate::log::OutputSink>>,
    clock: Arc<dyn Clock>,
    id_source: Arc<dyn IdSource>,
}

impl Default for RunnerBuilder {
    fn default() -> Self {
        Self {
            queue: None,
            workspaces: None,
            agent: None,
            prompt_selector: None,
            events: EventDispatcher::new(),
            observers: Vec::new(),
            config: RunnerPolicy::default(),
            stdio_sink: None,
            clock: Arc::new(SystemClock),
            id_source: Arc::new(SystemIdSource),
        }
    }
}

impl RunnerBuilder {
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
    pub fn queue(mut self, queue: Arc<dyn Queue>) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Supply the per-iteration workspace supply used to mint a fresh
    /// workspace for each signal.
    ///
    /// The supply yields a `Box<dyn Workspace>` so the runtime workspace axis
    /// is a trait object (R18); the closed set of workspace kinds lives at the
    /// definition layer, not here.
    pub fn workspaces<F>(mut self, supply: F) -> Self
    where
        F: Fn() -> Box<dyn Workspace> + Send + Sync + 'static,
    {
        self.workspaces = Some(Arc::new(supply));
        self
    }

    /// Supply the [`Agent`] used for every iteration.
    ///
    /// The agent is a trait object (`Box<dyn Agent>`): the closed set of
    /// agent kinds lives at the definition layer, and the runtime drives a
    /// single boxed agent (R18). The operator's translation fn boxes the
    /// concrete driver it selects from the agent definition; standalone
    /// callers box the agent themselves.
    pub fn agent(mut self, agent: Box<dyn Agent>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Supply the [`PromptSelector`] used to render prompts.
    ///
    /// Prefer this method when the declaration includes guarded prompts;
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

    /// Replace the runner's [`RunnerPolicy`].
    pub fn config(mut self, config: RunnerPolicy) -> Self {
        self.config = config;
        self
    }

    /// Register an [`EventAction`] for a specific [`EventName`].
    ///
    /// The handler is only invoked when the emitter dispatches an event
    /// whose [`HookEvent::name`](crate::runner::HookEvent::name) matches.
    pub fn on<H>(mut self, name: EventName, handler: H) -> Self
    where
        H: EventAction + 'static,
    {
        self.events.on(name, handler);
        self
    }

    /// Register an [`EventAction`] for every [`EventName`].
    ///
    /// The handler must be [`Clone`] because it is registered once per
    /// event name. Useful for test capture handlers and cross-cutting
    /// concerns like logging.
    pub fn on_all<H>(self, handler: H) -> Self
    where
        H: EventAction + Clone + 'static,
    {
        let mut this = self;
        for &name in EventName::ALL {
            this.events.on(name, handler.clone());
        }
        this
    }

    /// Replace the [`EventDispatcher`] wholesale.
    pub fn events(mut self, events: EventDispatcher) -> Self {
        self.events = events;
        self
    }

    /// Register a [`RunnerObserver`] for the system observer stream.
    ///
    /// Observers run **before** the user-defined [`EventDispatcher`] handlers
    /// at every runner step (rev17 §F3) so a user-installed
    /// `on workspace_teardown_finished { shell "..." }` cannot mask the system
    /// observer contract that backs the per-process log sink. Failures are
    /// best-effort — they are tallied into the terminal `runner_finished`
    /// event and logged via `tracing` at `warn` level, but do not halt the
    /// loop.
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
    /// operator's start path supplies this from
    /// `ProcessRuntime::sink()` so agent output reaches the per-process
    /// `log.ndjson`. Standalone runners may leave it unset — the runner
    /// falls back to a [`NoopSink`](crate::log::NoopSink) in that case.
    pub fn stdio_sink(mut self, sink: Arc<dyn crate::log::OutputSink>) -> Self {
        self.stdio_sink = Some(sink);
        self
    }

    /// Supply the [`Clock`] used for runner lifecycle timestamps and
    /// synthesized signals.
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Supply the [`IdSource`] used for synthesized signal identifiers.
    pub fn id_source(mut self, id_source: Arc<dyn IdSource>) -> Self {
        self.id_source = id_source;
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
    /// directly — the `Agent` trait has no associated error type — and the
    /// agent is a `Box<dyn Agent>`, so [`Runner`] is a concrete type with no
    /// parameters.
    pub fn build(self) -> Result<Runner, BuilderError> {
        let workspaces = self
            .workspaces
            .ok_or(BuilderError::MissingField("workspaces"))?;
        let agent = self.agent.ok_or(BuilderError::MissingField("agent"))?;
        let prompt_selector = self
            .prompt_selector
            .ok_or(BuilderError::MissingField("prompt_selector"))?;

        if self.queue.is_none() && matches!(self.config.behavior, SignalAcquisition::Wait) {
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
            clock: self.clock,
            id_source: self.id_source,
        })
    }
}
