//! Public AST for the iter workflow definition language.
//!
//! # Design rule: iter takes no project-shaped decisions
//!
//! Every field below that represents a *project-shaped* decision is required.
//! iter is a generic agent control framework — it must not silently fill in
//! ecosystem-specific defaults ("skip `target` and `node_modules`", "invoke
//! `claude` from `PATH`", "deny all network", "store the queue under
//! `./.iter/`"). Those are the project author's calls, not iter's.
//!
//! The language therefore spells each such knob as a required field. The only
//! `Option<T>` fields are the ones whose `None` is semantically distinct from
//! "iter picks for you" (e.g. `remote` on a clone — the project opts into
//! fetching from an external source or not).
//!
//! # The two kinds of knowledge iter holds
//!
//! 1. **Agent operational knowledge** — "what does Claude Code need to
//!    function?" is shipped inside iter (see `iter_core::Agent::sandbox_requirements`).
//!    Users do not enumerate per-agent paths or hosts in source files.
//! 2. **Project-shaped decisions** — expressed in the source file. Every field in this module.
//!
//! The two are merged at workspace setup: the project's `SandboxPolicyDecl`
//! is the upper bound, the agent's requirements are the lower bound. See
//! `iter_core::SandboxWorkspace` for the merge semantics.
//!
//! Every type in this module is part of the v1 stability contract. Refer to
//! `docs/dsl-reference.md` for the corresponding surface syntax and the
//! semantic rules that govern each variant.

mod agent;
mod arg;
mod compose;
mod event;
mod prompt;
mod queue;
mod runner;
mod telemetry;
mod trigger;
mod value;
mod workspace;

pub use agent::{AgentDecl, AgentMode};
pub use arg::ArgDecl;
pub use compose::{
    ComposeRoot, ComposeServiceOverride, ComposeTriggerOverride, InlineService, NamedCompose,
    NamedQueue, NamedService, NamedTrigger, QueueRef, ServiceSource,
};
pub use event::{Action, EventHandlerDecl, EventName};
pub use prompt::{CmpOp, IterationField, PriorityKeyword, PromptDecl, PromptGuard};
pub use queue::{
    DlqPolicyDecl, DlqTargetDecl, KafkaConfig, KafkaConsumer, KafkaProducer, KafkaSecurity,
    KinesisCheckpoint, KinesisConfig, KinesisConsumer, KinesisIdentity, KinesisProducer,
    KinesisShardListFilter, PubSubConfig, PubSubCredentialKind, PubSubCredentials,
    PubSubInitialSeek, PubSubKeepalive, PubSubPublisher, PubSubSubscriber, QueueDecl,
    RetryPolicyDecl, ServiceBusAuth, ServiceBusAuthKind, ServiceBusConfig, ServiceBusProxy,
    ServiceBusReceiver, ServiceBusSender, ServiceBusSession, SqsConfig, SqsConsumer,
    SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity, SqsProducer, TemplatedString,
};
pub use runner::{RunnerBehavior, RunnerDecl};
pub use telemetry::{TelemetryDecl, TelemetryProtocol};
pub use trigger::{
    ExtractExpr, FilesSource, OnErrorKeyword, SecretExpr, TriggerDecl, WatchEventKind,
    WebhookRoute,
};
pub use value::Value;
pub use workspace::{
    ApplyBackDecl, CloneApplyBackMode, SandboxNetworkDecl, SandboxPolicyDecl, WorkspaceDecl,
};

/// Inclusive-exclusive byte range inside the original source text.
pub type Span = std::ops::Range<usize>;

/// Wrapper that attaches a source [`Span`] to any AST node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    /// The underlying AST node.
    pub node: T,
    /// Byte range inside the original source that produced [`Spanned::node`].
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Construct a new [`Spanned`] from a node and its span.
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    /// Map the contained node while preserving the span.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }
}

/// A fully-parsed and semantically validated root of the iter language AST.
///
/// All top-level sections are optional because the language permits "partial"
/// files: webhook handlers may omit `workspace`/`agent`, while worker files
/// may omit `runner`. Only the well-formedness of *present* sections is
/// enforced.
///
/// The Iterfile model has no top-level `trigger` section — triggers live in
/// `compose.iter`. A `trigger { ... }` block at Iterfile root is a semantic
/// error guiding the user toward `compose.iter`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Root {
    /// `arg <name> [= "<default>"]` declarations in source order.
    pub args: Vec<Spanned<ArgDecl>>,
    /// `queue <kind> { ... }` declaration, if present.
    pub queue: Option<Spanned<QueueDecl>>,
    /// `workspace <kind> { ... }` declaration, if present.
    pub workspace: Option<Spanned<WorkspaceDecl>>,
    /// `agent <kind> { ... }` declaration, if present.
    pub agent: Option<Spanned<AgentDecl>>,
    /// `runner { ... }` declaration, if present.
    pub runner: Option<Spanned<RunnerDecl>>,
    /// All `prompt [when ...] "..."` declarations in source order.
    pub prompts: Vec<Spanned<PromptDecl>>,
    /// All top-level `on <event-name> { ... }` event handler declarations
    /// (NOT the per-route handlers nested inside `trigger webhook`).
    pub events: Vec<Spanned<EventHandlerDecl>>,
}
