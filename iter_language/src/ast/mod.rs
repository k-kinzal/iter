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
//!    function?" is shipped inside iter as each agent's own requirements.
//!    Users do not enumerate per-agent paths or hosts in source files.
//! 2. **Project-shaped decisions** — expressed in the source file. Every field in this module.
//!
//! The two are merged at workspace setup: the project's `SandboxPolicyDef`
//! is the upper bound and the agent's requirements are the lower bound. This
//! module describes only the project-shaped half — the declaration.
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

pub use agent::{AgentDef, AgentMode, RouterStrategy};
pub use arg::ArgDef;
pub use compose::{
    Compose, ComposeServiceOverride, ComposeTriggerOverride, InlineService, NamedCompose,
    NamedQueue, NamedService, NamedTrigger, QueueRef, ServiceSource,
};
pub use event::{Action, EventHandlerDef, EventName};
pub use prompt::{
    CmpOp, IterationField, NamedPrompt, PriorityKeyword, PromptArm, PromptDef, PromptExpr,
    PromptGuard, PromptValue,
};
pub use queue::{
    DlqPolicyDef, DlqTargetDef, MetadataSource, QueueDef, RetryPolicyDef, SqsConfig, SqsConsumer,
    SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity, SqsProducer,
};
pub use runner::{RunnerBehavior, RunnerDef};
pub use telemetry::{TelemetryDef, TelemetryProtocol};
pub use trigger::{
    ExtractExpr, FilesSource, OnErrorKeyword, SecretExpr, Subscription, TriggerDef, WatchEventKind,
};
pub use value::Value;
pub use workspace::{
    ApplyBackDef, CloneApplyBackMode, SandboxNetworkDef, SandboxPolicyDef, WorkspaceDef,
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

/// Wrapper that pairs a named identifier with its declaration.
///
/// Used for top-level definitions that carry a user-facing name:
/// `agent claude as primary { ... }` → `NamedDef { name: "primary", decl: AgentDef::Claude { ... } }`.
/// When the `as <name>` clause is omitted, the kind doubles as the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedDef<T> {
    /// User-facing name. Defaults to the kind when `as` is absent.
    pub name: String,
    /// The underlying declaration.
    pub decl: T,
}

/// A fully-parsed and semantically validated root of the iter language AST.
///
/// Definitions are named and stored in vectors (multiple of each kind
/// allowed). Runners bind definitions by name.
///
/// The Iterfile model has no top-level `trigger` section — triggers live in
/// `compose.iter`. A `trigger { ... }` block at Iterfile root is a semantic
/// error guiding the user toward `compose.iter`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Iterfile {
    /// `arg <name> [= "<default>"]` declarations in source order.
    pub args: Vec<Spanned<ArgDef>>,
    /// Named queue definitions: `queue <kind> [as <name>] { ... }`.
    pub queues: Vec<Spanned<NamedDef<QueueDef>>>,
    /// Named workspace definitions: `workspace <kind> [as <name>] { ... }`.
    pub workspaces: Vec<Spanned<NamedDef<WorkspaceDef>>>,
    /// Named agent definitions: `agent <kind> [as <name>] { ... }`.
    pub agents: Vec<Spanned<NamedDef<AgentDef>>>,
    /// Named prompt templates: `prompt as <name> "..."`.
    pub prompts: Vec<Spanned<NamedPrompt>>,
    /// Runner declarations (each binds definitions by reference).
    pub runners: Vec<Spanned<RunnerDef>>,
}
