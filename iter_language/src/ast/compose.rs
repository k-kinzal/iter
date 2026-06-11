//! `compose.iter` AST.
//!
//! `compose.iter` is the orchestration layer that wires together one or more
//! services (each modelled by an `Iterfile`) and one or more triggers via
//! shared queues. Where the [`Iterfile`](super::Iterfile) AST corresponds to a single
//! self-contained iter unit, [`Compose`] corresponds to the multi-unit
//! deployment topology a Docker `compose.yml` would describe.
//!
//! # Top-level shape
//!
//! Five sections are recognised:
//!
//! - `queue <name> <kind> { ... }` — named queue declaration. The kind +
//!   field grammar is identical to the Iterfile [`QueueDef`](super::QueueDef).
//! - `service <name> { ... }` — named service. The body either points at an
//!   external Iterfile via `build = "./Iterfile"` or inlines the same
//!   sections an Iterfile would carry (`workspace`, `agent`, `runner`). As
//!   in the new Iterfile design, the prompt expression and `on <event>`
//!   lifecycle handlers live inside the inline `runner` block rather than
//!   as independent sections.
//! - `trigger <name> <kind> { ... }` — named trigger. Body uses the same
//!   per-kind grammar as the Iterfile [`TriggerDef`](super::TriggerDef)
//!   plus a required `target = <queue-name>` (omittable when there is a
//!   single queue in the compose file).
//! - `compose <name> { ... }` — reference to another `compose.iter` file.
//!   The child's queues, services, and triggers are flattened into the
//!   parent plan. Optional `queues`, `services`, and `triggers` override
//!   blocks can rebind or disable child declarations.
//! - `telemetry { ... }` — optional project-wide OpenTelemetry export
//!   settings for the composed topology.
//!
//! `runner` only appears nested inside an inline service body — it is not a
//! first-class compose section. The runner's prompt and `on <event>`
//! handlers are nested inside that runner block.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{AgentDef, QueueDef, RunnerDef, Spanned, TelemetryDef, TriggerDef, WorkspaceDef};

/// Validated root of a `compose.iter` source file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compose {
    /// Optional project-wide telemetry declaration.
    pub telemetry: Option<Spanned<TelemetryDef>>,
    /// Named queue declarations in source order.
    pub queues: Vec<Spanned<NamedQueue>>,
    /// Named service declarations in source order.
    pub services: Vec<Spanned<NamedService>>,
    /// Named trigger declarations in source order.
    pub triggers: Vec<Spanned<NamedTrigger>>,
    /// Nested compose references in source order.
    pub composes: Vec<Spanned<NamedCompose>>,
}

/// One named entry in the compose file's `queue` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedQueue {
    /// User-facing identifier other sections refer to via `queue = <name>` /
    /// `target = <name>`.
    pub name: String,
    /// Backend declaration. Re-uses the Iterfile [`QueueDef`] grammar.
    pub decl: QueueDef,
}

/// One named entry in the compose file's `service` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedService {
    /// User-facing identifier.
    pub name: String,
    /// Where the runner-side configuration comes from.
    pub source: ServiceSource,
}

/// Origin of a service's runner-side configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceSource {
    /// `build = "./Iterfile"` — the service is defined by an external Iterfile.
    /// The compose-level `queue` field overrides any queue declared in the
    /// referenced Iterfile.
    Build {
        /// Path to the Iterfile, resolved relative to the compose file.
        path: PathBuf,
        /// Compose-level queue binding. `None` means "use the single queue in
        /// this compose file" (a `compose.iter`-validation error otherwise).
        queue: Option<QueueRef>,
        /// Arg overrides passed to the referenced Iterfile. Overrides
        /// Iterfile-level `arg` defaults at build time.
        args: BTreeMap<String, String>,
    },
    /// `service <name> { workspace ... agent ... runner ... }` — every
    /// runner-side section is declared inline. Boxed to keep
    /// [`ServiceSource`] variants close in size.
    Inline(Box<InlineService>),
}

/// Inline service body. Carried behind a [`Box`] inside
/// [`ServiceSource::Inline`] so the enum's variants stay similarly sized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineService {
    /// Compose-level queue binding.
    pub queue: Option<QueueRef>,
    /// Inline workspace declaration.
    pub workspace: Option<Spanned<WorkspaceDef>>,
    /// Inline agent declaration.
    pub agent: Option<Spanned<AgentDef>>,
    /// Inline runner declaration. In the new design this carries the
    /// service's prompt expression (`prompt = ...`) and lifecycle event
    /// handlers (`on <event> { ... }`) directly, binding them to the
    /// runner rather than holding them as independent top-level sections.
    pub runner: Option<Spanned<RunnerDef>>,
}

/// One named entry in the compose file's `trigger` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTrigger {
    /// User-facing identifier.
    pub name: String,
    /// Trigger configuration. Re-uses the Iterfile [`TriggerDef`] grammar
    /// minus the `loop` variant (now expressed as `runner.behavior = loop`).
    pub decl: TriggerDef,
    /// Queue this trigger emits signals into.
    pub target: QueueRef,
    /// When `true`, the trigger enqueues a terminate signal on its
    /// target queue after it finishes emitting. Default is `false`.
    pub terminate_on_completion: bool,
}

/// Reference to a named queue inside a compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueRef {
    /// `queue = <name>` / `target = <name>` — explicit binding to a named
    /// queue declared elsewhere in the file.
    Named(String),
    /// The reference was omitted; the semantic layer auto-resolved it to the
    /// only queue in the file. Errors otherwise.
    Anonymous,
}

/// One named entry in the compose file's `compose` section — a reference
/// to another `compose.iter` file whose queues, services, and triggers
/// are flattened into the parent plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedCompose {
    /// User-facing identifier for this compose reference.
    pub name: String,
    /// Path to the child `compose.iter` file, resolved relative to the
    /// parent compose file.
    pub path: PathBuf,
    /// Queue overrides: maps a child queue name to a parent queue
    /// reference. The child queue is discarded and the parent queue is
    /// used instead.
    pub queues: BTreeMap<String, QueueRef>,
    /// Service overrides: maps a child service name to attribute
    /// overrides (e.g. queue rebinding).
    pub services: BTreeMap<String, ComposeServiceOverride>,
    /// Trigger overrides: maps a child trigger name to an override or
    /// disablement.
    pub triggers: BTreeMap<String, ComposeTriggerOverride>,
}

/// Attribute overrides for a child service imported via a `compose`
/// block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeServiceOverride {
    /// Override the child service's queue binding.
    pub queue: Option<QueueRef>,
}

/// Override or disablement for a child trigger imported via a `compose`
/// block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeTriggerOverride {
    /// Disable this trigger entirely — it will not appear in the
    /// flattened plan.
    Disabled,
    /// Override specific trigger attributes.
    Override {
        /// Override the trigger's target queue.
        target: Option<QueueRef>,
    },
}
