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
use crate::runner::{
    CompletionCondition, EventAction, EventDispatcher, Runner, RunnerPolicy, SignalAcquisition,
};
use crate::time::{Clock, IdSource, SystemClock, SystemIdSource};
use crate::variable::VariableStore;
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
/// The runner binds one [`Workspace`] (held as `Box<dyn Workspace>` for the
/// whole exploration) with one [`Agent`] (a concrete struct — the cycle is
/// implemented once; what varies lives in its router and drivers), so
/// `RunnerBuilder` carries no type parameters.
#[must_use = "call `build()` to produce a Runner"]
pub struct RunnerBuilder {
    queue: Option<Arc<dyn Queue>>,
    workspace: Option<Box<dyn Workspace>>,
    agent: Option<Agent>,
    prompt_selector: Option<PromptSelector>,
    events: EventDispatcher,
    observers: Vec<Arc<dyn DynRunnerObserver>>,
    config: RunnerPolicy,
    completion_conditions: Vec<CompletionCondition>,
    stdio_sink: Option<Arc<dyn crate::log::OutputSink>>,
    clock: Arc<dyn Clock>,
    id_source: Arc<dyn IdSource>,
    variables: VariableStore,
}

impl Default for RunnerBuilder {
    fn default() -> Self {
        Self {
            queue: None,
            workspace: None,
            agent: None,
            prompt_selector: None,
            events: EventDispatcher::new(),
            observers: Vec::new(),
            config: RunnerPolicy::default(),
            completion_conditions: Vec::new(),
            stdio_sink: None,
            clock: Arc::new(SystemClock),
            id_source: Arc::new(SystemIdSource),
            variables: VariableStore::new(),
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

    /// Supply the single [`Workspace`] the runner holds for the whole
    /// exploration. Each iteration calls
    /// [`setup`](crate::workspace::Workspace::setup) on it to mint a fresh
    /// [`ActiveWorkspace`](crate::workspace::ActiveWorkspace); the runtime
    /// workspace axis stays a trait object (R18) — the closed set of
    /// workspace kinds lives at the definition layer, not here.
    pub fn workspace(mut self, workspace: Box<dyn Workspace>) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// Supply the [`Agent`] bound to this runner for every iteration.
    ///
    /// `Agent` is a concrete struct: the cycle is implemented once. What
    /// varies per CLI lives in its drivers, and what varies per composition
    /// in its router — both fixed when the operator's translation layer
    /// assembles the agent from its definition.
    pub fn agent(mut self, agent: Agent) -> Self {
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

    /// Replace the ordered OR-set of first-class completion conditions.
    pub fn completion_conditions(mut self, conditions: Vec<CompletionCondition>) -> Self {
        self.completion_conditions = conditions;
        self
    }

    /// Clone the configured event dispatcher for deferred operator-owned
    /// lifecycle events such as `runner_completed`.
    #[must_use]
    pub fn event_dispatcher(&self) -> EventDispatcher {
        self.events.clone()
    }

    /// Clone the Runner-scoped dynamic variable store.
    ///
    /// Operator actions use this handle to publish `var.*` values; the
    /// Runner keeps the same store for prompt rendering.
    #[must_use]
    pub fn variable_store(&self) -> VariableStore {
        self.variables.clone()
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

    /// Install the [`OutputSink`](crate::log::OutputSink) the agent tees its
    /// child stdout/stderr through. Fixed onto the [`Agent`] at
    /// [`build`](Self::build) time — completing the agent's birth — so the
    /// operator's start path can supply it from `ProcessRuntime::sink()`
    /// after the agent was assembled from its definition. Standalone
    /// runners may leave it unset — the agent keeps its
    /// [`NoopSink`](crate::log::NoopSink) default in that case.
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
    ///
    /// Building is where the agent's birth completes: a sink installed via
    /// [`Self::stdio_sink`] is fixed onto the agent here (replacing the
    /// agent's no-op default), so the running agent never carries the sink
    /// through any call signature.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::MissingField`] when a required collaborator
    /// was not supplied, and [`BuilderError::InvalidConfig`] for an
    /// internally inconsistent configuration.
    pub fn build(self) -> Result<Runner, BuilderError> {
        let workspace = self
            .workspace
            .ok_or(BuilderError::MissingField("workspace"))?;
        let mut agent = self.agent.ok_or(BuilderError::MissingField("agent"))?;
        let prompt_selector = self
            .prompt_selector
            .ok_or(BuilderError::MissingField("prompt_selector"))?;

        if self.queue.is_none() && matches!(self.config.behavior, SignalAcquisition::Wait) {
            return Err(BuilderError::InvalidConfig(
                "behavior = wait requires a queue declaration",
            ));
        }

        if let Some(sink) = self.stdio_sink {
            agent = agent.with_stdio_sink(sink);
        }

        Ok(Runner {
            queue: self.queue,
            workspace,
            agent,
            prompt_selector,
            events: self.events,
            observers: self.observers,
            config: self.config,
            completion_conditions: self.completion_conditions,
            clock: self.clock,
            id_source: self.id_source,
            variables: self.variables,
        })
    }
}
